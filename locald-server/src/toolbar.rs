pub const TOOLBAR_JS: &str = r#"
(function() {
    // Create FAB
    const fab = document.createElement('div');
    fab.id = 'locald-fab';
    fab.innerHTML = '⚡';
    Object.assign(fab.style, {
        position: 'fixed',
        bottom: '20px',
        right: '20px',
        width: '50px',
        height: '50px',
        borderRadius: '25px',
        backgroundColor: '#2563eb',
        color: 'white',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: '24px',
        cursor: 'pointer',
        boxShadow: '0 4px 6px -1px rgba(0, 0, 0, 0.1)',
        zIndex: '9999',
        transition: 'transform 0.2s, background-color 0.2s',
        userSelect: 'none',
    });

    // Create Toolbar Container (Hidden by default)
    const toolbar = document.createElement('div');
    toolbar.id = 'locald-toolbar';
    Object.assign(toolbar.style, {
        position: 'fixed',
        bottom: '80px',
        right: '20px',
        width: '300px',
        maxHeight: '400px',
        backgroundColor: '#18181b',
        color: '#f4f4f5',
        borderRadius: '8px',
        boxShadow: '0 10px 15px -3px rgba(0, 0, 0, 0.1)',
        zIndex: '9999',
        display: 'none',
        flexDirection: 'column',
        overflow: 'hidden',
        fontFamily: 'system-ui, sans-serif',
        fontSize: '14px',
        border: '1px solid #27272a',
    });

    // Header
    const header = document.createElement('div');
    Object.assign(header.style, {
        padding: '12px',
        backgroundColor: '#27272a',
        borderBottom: '1px solid #3f3f46',
        fontWeight: '600',
        display: 'flex',
        justifyContent: 'space-between',
        alignItems: 'center',
    });
    header.innerHTML = '<span>Build Status</span><span id="locald-status-text" style="font-size: 12px; color: #a1a1aa;">Connecting...</span>';
    toolbar.appendChild(header);

    // Logs Area
    const logs = document.createElement('div');
    logs.id = 'locald-logs';
    Object.assign(logs.style, {
        padding: '12px',
        overflowY: 'auto',
        flexGrow: '1',
        fontFamily: 'monospace',
        whiteSpace: 'pre-wrap',
        fontSize: '12px',
        color: '#d4d4d8',
    });
    toolbar.appendChild(logs);

    document.body.appendChild(fab);
    document.body.appendChild(toolbar);

    // Toggle Logic
    let isOpen = false;
    fab.addEventListener('click', () => {
        isOpen = !isOpen;
        toolbar.style.display = isOpen ? 'flex' : 'none';
        fab.style.transform = isOpen ? 'rotate(45deg)' : 'rotate(0deg)';
    });

    // Status Management
    function setStatus(status) {
        const statusText = document.getElementById('locald-status-text');
        switch (status) {
            case 'building':
                fab.style.backgroundColor = '#eab308'; // Yellow
                fab.innerHTML = '⚡';
                statusText.textContent = 'Building...';
                statusText.style.color = '#fef08a';
                break;
            case 'success':
                fab.style.backgroundColor = '#22c55e'; // Green
                fab.innerHTML = '✓';
                statusText.textContent = 'Ready';
                statusText.style.color = '#bbf7d0';
                break;
            case 'failed':
                fab.style.backgroundColor = '#ef4444'; // Red
                fab.innerHTML = '!';
                statusText.textContent = 'Build Failed';
                statusText.style.color = '#fecaca';
                // Auto-open on failure
                if (!isOpen) fab.click();
                break;
            default:
                fab.style.backgroundColor = '#71717a'; // Gray
                statusText.textContent = status;
        }
    }

    // WebSocket Connection
    function connect() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const ws = new WebSocket(`${protocol}//${window.location.host}/__locald/ws`);

        ws.onopen = () => {
            console.log('[locald] Connected');
            setStatus('success'); // Assume success initially or wait for message
        };

        ws.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                // Handle LogEntry
                if (data.message) {
                    const line = document.createElement('div');
                    line.textContent = data.message;
                    if (data.stream === 'stderr') {
                        line.style.color = '#fca5a5';
                    }
                    logs.appendChild(line);
                    logs.scrollTop = logs.scrollHeight;

                    // Heuristic status updates based on logs (temporary until structured events)
                    if (data.message.includes('Running build')) setStatus('building');
                    if (data.message.includes('Build succeeded')) setStatus('success');
                    if (data.message.includes('Build failed')) setStatus('failed');
                }
            } catch (e) {
                console.error('[locald] Failed to parse message', e);
            }
        };

        ws.onclose = () => {
            console.log('[locald] Disconnected');
            setStatus('Disconnected');
            setTimeout(connect, 2000);
        };
    }

    connect();
})();
"#;
