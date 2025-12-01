const state = {
    services: [],
    logs: [],
    connected: false,
};

const el = {
    status: document.getElementById('connection-status'),
    serviceList: document.getElementById('service-list'),
    logViewer: document.getElementById('log-viewer'),
};

function updateStatus(connected) {
    state.connected = connected;
    if (connected) {
        el.status.textContent = 'Connected';
        el.status.className = 'status-indicator connected';
    } else {
        el.status.textContent = 'Disconnected (Retrying...)';
        el.status.className = 'status-indicator disconnected';
    }
}

async function fetchServices() {
    try {
        const res = await fetch('/api/state');
        if (!res.ok) throw new Error('Failed to fetch state');
        const services = await res.json();
        state.services = services;
        renderServices();
    } catch (e) {
        console.error(e);
    }
}

function renderServices() {
    if (state.services.length === 0) {
        el.serviceList.innerHTML = '<div style="padding:1rem; color:#666;">No services running</div>';
        return;
    }
    
    el.serviceList.innerHTML = state.services.map(s => `
        <div class="service-card">
            <div class="service-header">
                <span class="service-name">${s.name}</span>
                <span class="service-status status-${s.status}">${s.status}</span>
            </div>
            <div class="service-details">
                ${s.port ? `Port: ${s.port}` : ''}
                ${s.url ? `<br><a href="${s.url}" target="_blank" class="service-link">${s.url}</a>` : ''}
            </div>
        </div>
    `).join('');
    el.serviceList.classList.remove('loading');
}

function renderLog(entry) {
    const div = document.createElement('div');
    div.className = `log-entry log-stream-${entry.stream}`;
    
    const time = new Date(entry.timestamp * 1000).toLocaleTimeString();
    
    div.innerHTML = `
        <span class="log-timestamp">${time}</span>
        <span class="log-service">[${entry.service}]</span>
        <span class="log-message">${escapeHtml(entry.message)}</span>
    `;
    
    el.logViewer.appendChild(div);
    
    // Auto-scroll if near bottom
    const isNearBottom = el.logViewer.scrollHeight - el.logViewer.scrollTop - el.logViewer.clientHeight < 100;
    if (isNearBottom) {
        el.logViewer.scrollTop = el.logViewer.scrollHeight;
    }
}

function escapeHtml(text) {
    const map = {
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&#039;'
    };
    return text.replace(/[&<>"']/g, function(m) { return map[m]; });
}

let ws;
let reconnectDelay = 1000;

function connect() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/api/logs`;
    
    ws = new WebSocket(wsUrl);

    ws.onopen = () => {
        console.log('Connected to logs');
        updateStatus(true);
        reconnectDelay = 1000;
        fetchServices(); // Refresh services on connect
    };

    ws.onmessage = (event) => {
        try {
            const entry = JSON.parse(event.data);
            renderLog(entry);
        } catch (e) {
            console.error('Failed to parse log entry', e);
        }
    };

    ws.onclose = () => {
        console.log('Disconnected');
        updateStatus(false);
        setTimeout(connect, reconnectDelay);
        reconnectDelay = Math.min(reconnectDelay * 1.5, 10000);
    };

    ws.onerror = (err) => {
        console.error('WebSocket error', err);
        ws.close();
    };
}

// Initial load
fetchServices();
connect();

// Poll services every 5s just in case
setInterval(fetchServices, 5000);
