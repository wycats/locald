use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::manager::ProcessManager;

pub struct ProxyManager {
    process_manager: ProcessManager,
}

impl ProxyManager {
    pub fn new(process_manager: ProcessManager) -> Self {
        Self { process_manager }
    }

    pub async fn start(&self, port: u16) -> anyhow::Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = TcpListener::bind(addr).await?;
        info!("Proxy listening on http://{}", addr);

        let pm = self.process_manager.clone();
        let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());
        let client = Arc::new(client);

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let pm = pm.clone();
            let client = client.clone();

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service_fn(move |req| proxy_request(req, pm.clone(), client.clone())))
                    .await
                {
                    error!("Error serving connection: {:?}", err);
                }
            });
        }
    }
}

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}

async fn proxy_request(
    req: Request<hyper::body::Incoming>,
    pm: ProcessManager,
    client: Arc<hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, hyper::body::Incoming>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let host = match req.headers().get("host") {
        Some(h) => h.to_str().unwrap_or_default().split(':').next().unwrap_or_default(),
        None => return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(full("Missing Host header"))
            .unwrap()),
    };

    if let Some(port) = pm.find_port_by_domain(host).await {
        let uri_string = format!("http://127.0.0.1:{}{}", port, req.uri().path_and_query().map(|x| x.as_str()).unwrap_or(""));
        let uri: Uri = uri_string.parse().unwrap();
        
        let mut new_req = Request::builder()
            .method(req.method())
            .uri(uri)
            .version(req.version());
            
        for (key, value) in req.headers() {
            // Don't forward host header? Or do we?
            // Usually we want to preserve it or set it to the target.
            // But the target is localhost, so maybe it doesn't matter.
            // However, if the app checks Host header, it should match what the browser sent.
            if key.as_str() != "host" {
                 new_req.headers_mut().unwrap().insert(key, value.clone());
            }
        }
        // Set host header to localhost? Or keep original?
        // If we keep original, the app might be confused if it's not configured for it.
        // But usually we want to pass the original Host header.
        new_req.headers_mut().unwrap().insert("host", host.parse().unwrap());
        
        let new_req = new_req.body(req.into_body()).unwrap();
        
        match client.request(new_req).await {
            Ok(res) => {
                let (parts, body) = res.into_parts();
                let boxed_body = body.map_err(|e| e).boxed();
                Ok(Response::from_parts(parts, boxed_body))
            }
            Err(e) => {
                error!("Proxy error: {}", e);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(full(format!("Proxy error: {}", e)))
                    .unwrap())
            }
        }
    } else {
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full(format!("Domain {} not found", host)))
            .unwrap())
    }
}
