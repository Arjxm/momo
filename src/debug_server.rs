//! Debug web server for graph memory visualization
//!
//! Provides a web interface to visualize the graph database,
//! including tools, memories, episodes, and topics.

use std::sync::Arc;
use warp::Filter;

use crate::graph::GraphBrain;

/// Start the debug server on the specified port
pub async fn start_debug_server(brain: Arc<GraphBrain>, port: u16) {
    let brain_filter = warp::any().map(move || brain.clone());

    // Serve the main visualization page
    let index = warp::path::end()
        .map(|| warp::reply::html(INDEX_HTML));

    // API: Get graph stats
    let stats = warp::path!("api" / "stats")
        .and(brain_filter.clone())
        .map(|brain: Arc<GraphBrain>| {
            match brain.stats() {
                Ok(stats) => warp::reply::json(&serde_json::json!({
                    "tools": stats.tools,
                    "memories": stats.memories,
                    "episodes": stats.episodes,
                    "topics": stats.topics,
                })),
                Err(e) => warp::reply::json(&serde_json::json!({
                    "error": e.to_string()
                })),
            }
        });

    // API: Get all memories
    let memories = warp::path!("api" / "memories")
        .and(brain_filter.clone())
        .map(|brain: Arc<GraphBrain>| {
            match brain.get_all_memories(100) {
                Ok(memories) => {
                    println!("📊 [DEBUG] Returning {} memories", memories.len());
                    let data: Vec<_> = memories.iter().map(|m| {
                        serde_json::json!({
                            "id": m.id,
                            "content": m.content,
                            "memory_type": format!("{:?}", m.memory_type),
                            "importance": m.importance,
                            "created_at": m.created_at,
                            "fingerprint": m.fingerprint,
                            "source_task_id": m.source_task_id,
                        })
                    }).collect();
                    warp::reply::json(&data)
                }
                Err(e) => {
                    eprintln!("⚠️ [DEBUG] Error fetching memories: {}", e);
                    warp::reply::json(&serde_json::json!({
                        "error": e.to_string()
                    }))
                }
            }
        });

    // API: Get all episodes
    let episodes = warp::path!("api" / "episodes")
        .and(brain_filter.clone())
        .map(|brain: Arc<GraphBrain>| {
            match brain.recent_episodes("default", 50) {
                Ok(episodes) => {
                    println!("📊 [DEBUG] Returning {} episodes", episodes.len());
                    let data: Vec<_> = episodes.iter().map(|e| {
                        serde_json::json!({
                            "id": e.id,
                            "user_input": e.user_input,
                            "agent_response": e.agent_response,
                            "tools_used": e.tools_used,
                            "success": e.success,
                            "duration_ms": e.duration_ms,
                            "tokens_used": e.tokens_used,
                            "cost_usd": e.cost_usd,
                            "created_at": e.created_at,
                        })
                    }).collect();
                    warp::reply::json(&data)
                }
                Err(e) => {
                    eprintln!("⚠️ [DEBUG] Error fetching episodes: {}", e);
                    warp::reply::json(&serde_json::json!({
                        "error": e.to_string()
                    }))
                }
            }
        });

    // API: Get all tools
    let tools = warp::path!("api" / "tools")
        .and(brain_filter.clone())
        .map(|brain: Arc<GraphBrain>| {
            match brain.get_tools_by_type("native") {
                Ok(native_tools) => {
                    let mut all_tools = native_tools;
                    if let Ok(mcp_tools) = brain.get_tools_by_type("mcp") {
                        all_tools.extend(mcp_tools);
                    }
                    if let Ok(skill_tools) = brain.get_tools_by_type("skill") {
                        all_tools.extend(skill_tools);
                    }
                    let data: Vec<_> = all_tools.iter().map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "tool_type": format!("{:?}", t.tool_type),
                            "enabled": t.enabled,
                            "usage_count": t.usage_count,
                            "success_rate": t.success_rate,
                        })
                    }).collect();
                    warp::reply::json(&data)
                }
                Err(e) => warp::reply::json(&serde_json::json!({
                    "error": e.to_string()
                })),
            }
        });

    // API: Get graph data for visualization (nodes and edges)
    let graph_data = warp::path!("api" / "graph")
        .and(brain_filter.clone())
        .map(|brain: Arc<GraphBrain>| {
            let mut nodes: Vec<serde_json::Value> = Vec::new();
            let mut edges: Vec<serde_json::Value> = Vec::new();

            // Add tool nodes
            if let Ok(tools) = brain.get_tools_by_type("native") {
                for t in tools {
                    nodes.push(serde_json::json!({
                        "id": format!("tool:{}", t.name),
                        "label": t.name,
                        "type": "tool",
                        "group": "native"
                    }));
                }
            }
            if let Ok(tools) = brain.get_tools_by_type("mcp") {
                for t in tools {
                    nodes.push(serde_json::json!({
                        "id": format!("tool:{}", t.name),
                        "label": t.name,
                        "type": "tool",
                        "group": "mcp"
                    }));
                }
            }

            // Add memory nodes (limit to recent 50)
            match brain.get_all_memories(50) {
                Ok(memories) => {
                    for m in memories {
                        let short_content = if m.content.len() > 30 {
                            format!("{}...", &m.content[..30])
                        } else {
                            m.content.clone()
                        };
                        nodes.push(serde_json::json!({
                            "id": format!("memory:{}", m.id),
                            "label": short_content,
                            "type": "memory",
                            "group": format!("{:?}", m.memory_type)
                        }));
                    }
                }
                Err(e) => {
                    eprintln!("⚠️ [DEBUG] Error loading memories for graph: {}", e);
                }
            }

            warp::reply::json(&serde_json::json!({
                "nodes": nodes,
                "edges": edges
            }))
        });

    // API: Search memories
    let search = warp::path!("api" / "search")
        .and(warp::query::<std::collections::HashMap<String, String>>())
        .and(brain_filter.clone())
        .map(|params: std::collections::HashMap<String, String>, brain: Arc<GraphBrain>| {
            let query = params.get("q").map(|s| s.as_str()).unwrap_or("");
            let keywords: Vec<String> = query.split_whitespace().map(|s| s.to_string()).collect();

            match brain.recall(&keywords, 20) {
                Ok(memories) => {
                    let data: Vec<_> = memories.iter().map(|m| {
                        serde_json::json!({
                            "id": m.id,
                            "content": m.content,
                            "memory_type": format!("{:?}", m.memory_type),
                            "importance": m.importance,
                            "created_at": m.created_at,
                        })
                    }).collect();
                    warp::reply::json(&data)
                }
                Err(e) => warp::reply::json(&serde_json::json!({
                    "error": e.to_string()
                })),
            }
        });

    let routes = index
        .or(stats)
        .or(memories)
        .or(episodes)
        .or(tools)
        .or(graph_data)
        .or(search);

    println!();
    println!("🔍 Debug server running at http://localhost:{}", port);
    println!("   Open in browser to visualize graph memory");
    println!();

    warp::serve(routes).run(([127, 0, 0, 1], port)).await;
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Agent Brain - Graph Memory Visualization</title>
    <script src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: #1a1a2e;
            color: #eee;
        }
        .header {
            background: #16213e;
            padding: 1rem 2rem;
            display: flex;
            justify-content: space-between;
            align-items: center;
            border-bottom: 1px solid #0f3460;
        }
        .header h1 {
            font-size: 1.5rem;
            color: #e94560;
        }
        .stats {
            display: flex;
            gap: 2rem;
        }
        .stat {
            text-align: center;
        }
        .stat-value {
            font-size: 2rem;
            font-weight: bold;
            color: #e94560;
        }
        .stat-label {
            font-size: 0.8rem;
            color: #888;
        }
        .container {
            display: flex;
            height: calc(100vh - 80px);
        }
        .sidebar {
            width: 350px;
            background: #16213e;
            padding: 1rem;
            overflow-y: auto;
            border-right: 1px solid #0f3460;
        }
        .search-box {
            width: 100%;
            padding: 0.5rem;
            border: 1px solid #0f3460;
            border-radius: 4px;
            background: #1a1a2e;
            color: #eee;
            margin-bottom: 1rem;
        }
        .tabs {
            display: flex;
            gap: 0.5rem;
            margin-bottom: 1rem;
        }
        .tab {
            padding: 0.5rem 1rem;
            background: #0f3460;
            border: none;
            border-radius: 4px;
            color: #eee;
            cursor: pointer;
        }
        .tab.active {
            background: #e94560;
        }
        .list-item {
            padding: 0.75rem;
            background: #1a1a2e;
            border-radius: 4px;
            margin-bottom: 0.5rem;
            cursor: pointer;
            border: 1px solid transparent;
        }
        .list-item:hover {
            border-color: #e94560;
        }
        .list-item h3 {
            font-size: 0.9rem;
            margin-bottom: 0.25rem;
        }
        .list-item p {
            font-size: 0.8rem;
            color: #888;
        }
        .list-item .badge {
            display: inline-block;
            padding: 0.1rem 0.4rem;
            background: #0f3460;
            border-radius: 3px;
            font-size: 0.7rem;
            margin-right: 0.5rem;
        }
        .graph-container {
            flex: 1;
            position: relative;
        }
        #graph {
            width: 100%;
            height: 100%;
        }
        .legend {
            position: absolute;
            bottom: 1rem;
            left: 1rem;
            background: rgba(22, 33, 62, 0.9);
            padding: 1rem;
            border-radius: 4px;
            font-size: 0.8rem;
        }
        .legend-item {
            display: flex;
            align-items: center;
            gap: 0.5rem;
            margin-bottom: 0.25rem;
        }
        .legend-color {
            width: 12px;
            height: 12px;
            border-radius: 50%;
        }
        .detail-panel {
            display: none;
            position: absolute;
            top: 1rem;
            right: 1rem;
            width: 300px;
            background: rgba(22, 33, 62, 0.95);
            padding: 1rem;
            border-radius: 4px;
            border: 1px solid #0f3460;
        }
        .detail-panel.active {
            display: block;
        }
        .detail-panel h3 {
            margin-bottom: 0.5rem;
            color: #e94560;
        }
        .close-btn {
            position: absolute;
            top: 0.5rem;
            right: 0.5rem;
            background: none;
            border: none;
            color: #888;
            cursor: pointer;
            font-size: 1.2rem;
        }
    </style>
</head>
<body>
    <div class="header">
        <h1>🧠 Agent Brain - Graph Memory</h1>
        <div class="stats" id="stats">
            <div class="stat">
                <div class="stat-value" id="stat-tools">-</div>
                <div class="stat-label">Tools</div>
            </div>
            <div class="stat">
                <div class="stat-value" id="stat-memories">-</div>
                <div class="stat-label">Memories</div>
            </div>
            <div class="stat">
                <div class="stat-value" id="stat-episodes">-</div>
                <div class="stat-label">Episodes</div>
            </div>
            <div class="stat">
                <div class="stat-value" id="stat-topics">-</div>
                <div class="stat-label">Topics</div>
            </div>
        </div>
    </div>
    <div class="container">
        <div class="sidebar">
            <input type="text" class="search-box" id="search" placeholder="Search memories...">
            <div class="tabs">
                <button class="tab active" data-tab="memories">Memories</button>
                <button class="tab" data-tab="episodes">Episodes</button>
                <button class="tab" data-tab="tools">Tools</button>
            </div>
            <div id="list"></div>
        </div>
        <div class="graph-container">
            <div id="graph"></div>
            <div class="legend">
                <div class="legend-item"><div class="legend-color" style="background: #e94560;"></div> Tool (Native)</div>
                <div class="legend-item"><div class="legend-color" style="background: #f39c12;"></div> Tool (MCP)</div>
                <div class="legend-item"><div class="legend-color" style="background: #3498db;"></div> Memory</div>
                <div class="legend-item"><div class="legend-color" style="background: #2ecc71;"></div> Topic</div>
            </div>
            <div class="detail-panel" id="detail-panel">
                <button class="close-btn" onclick="closeDetail()">&times;</button>
                <h3 id="detail-title"></h3>
                <div id="detail-content"></div>
            </div>
        </div>
    </div>

    <script>
        let network;
        let currentTab = 'memories';

        // Color scheme
        const colors = {
            'native': '#e94560',
            'mcp': '#f39c12',
            'skill': '#9b59b6',
            'memory': '#3498db',
            'topic': '#2ecc71',
            'Fact': '#3498db',
            'Preference': '#9b59b6',
            'Procedure': '#e67e22',
        };

        // Load stats
        async function loadStats() {
            const res = await fetch('/api/stats');
            const data = await res.json();
            document.getElementById('stat-tools').textContent = data.tools || 0;
            document.getElementById('stat-memories').textContent = data.memories || 0;
            document.getElementById('stat-episodes').textContent = data.episodes || 0;
            document.getElementById('stat-topics').textContent = data.topics || 0;
        }

        // Load and render graph
        async function loadGraph() {
            const res = await fetch('/api/graph');
            const data = await res.json();

            const nodes = new vis.DataSet(data.nodes.map(n => ({
                id: n.id,
                label: n.label,
                color: colors[n.group] || '#888',
                shape: n.type === 'tool' ? 'diamond' : n.type === 'topic' ? 'dot' : 'box',
                size: n.type === 'topic' ? 15 : 20,
                font: { color: '#fff', size: 12 }
            })));

            const edges = new vis.DataSet(data.edges.map(e => ({
                from: e.from,
                to: e.to,
                color: { color: '#444', opacity: 0.5 },
                arrows: 'to'
            })));

            const container = document.getElementById('graph');
            const options = {
                physics: {
                    stabilization: { iterations: 100 },
                    barnesHut: {
                        gravitationalConstant: -2000,
                        springLength: 150
                    }
                },
                interaction: {
                    hover: true,
                    tooltipDelay: 200
                }
            };

            network = new vis.Network(container, { nodes, edges }, options);

            network.on('click', function(params) {
                if (params.nodes.length > 0) {
                    const nodeId = params.nodes[0];
                    showNodeDetail(nodeId, data.nodes.find(n => n.id === nodeId));
                }
            });
        }

        // Load memories, episodes, or tools list
        async function loadList(tab) {
            currentTab = tab;
            const list = document.getElementById('list');

            if (tab === 'memories') {
                const res = await fetch('/api/memories');
                const data = await res.json();
                if (data.error) {
                    list.innerHTML = `<div class="list-item"><p style="color:#e94560;">Error: ${data.error}</p></div>`;
                    return;
                }
                list.innerHTML = data.length === 0
                    ? '<div class="list-item"><p>No memories yet. Chat with the agent to create some!</p></div>'
                    : data.map(m => `
                    <div class="list-item" onclick="highlightNode('memory:${m.id}')">
                        <h3>${m.content.substring(0, 50)}${m.content.length > 50 ? '...' : ''}</h3>
                        <p>
                            <span class="badge">${m.memory_type}</span>
                            Importance: ${m.importance.toFixed(2)}
                        </p>
                    </div>
                `).join('');
            } else if (tab === 'episodes') {
                const res = await fetch('/api/episodes');
                const data = await res.json();
                if (data.error) {
                    list.innerHTML = `<div class="list-item"><p style="color:#e94560;">Error: ${data.error}</p></div>`;
                    return;
                }
                list.innerHTML = data.length === 0
                    ? '<div class="list-item"><p>No episodes yet. Start chatting!</p></div>'
                    : data.map(e => `
                    <div class="list-item">
                        <h3>${e.user_input.substring(0, 50)}${e.user_input.length > 50 ? '...' : ''}</h3>
                        <p>
                            <span class="badge">${e.success ? '✓' : '✗'}</span>
                            ${e.tools_used.length} tools | ${e.tokens_used} tokens | $${e.cost_usd.toFixed(4)}
                        </p>
                        <p style="font-size:0.7rem;color:#666;">${new Date(e.created_at).toLocaleString()}</p>
                    </div>
                `).join('');
            } else {
                const res = await fetch('/api/tools');
                const data = await res.json();
                if (data.error) {
                    list.innerHTML = `<div class="list-item"><p style="color:#e94560;">Error: ${data.error}</p></div>`;
                    return;
                }
                list.innerHTML = data.length === 0
                    ? '<div class="list-item"><p>No tools registered.</p></div>'
                    : data.map(t => `
                    <div class="list-item" onclick="highlightNode('tool:${t.name}')">
                        <h3>${t.name}</h3>
                        <p>
                            <span class="badge">${t.tool_type}</span>
                            Used: ${t.usage_count} | Success: ${(t.success_rate * 100).toFixed(0)}%
                        </p>
                    </div>
                `).join('');
            }
        }

        // Search memories
        async function searchMemories(query) {
            if (!query) {
                loadList('memories');
                return;
            }
            const res = await fetch(`/api/search?q=${encodeURIComponent(query)}`);
            const data = await res.json();
            const list = document.getElementById('list');
            list.innerHTML = data.map(m => `
                <div class="list-item" onclick="highlightNode('memory:${m.id}')">
                    <h3>${m.content.substring(0, 50)}${m.content.length > 50 ? '...' : ''}</h3>
                    <p>
                        <span class="badge">${m.memory_type}</span>
                        Importance: ${m.importance.toFixed(2)}
                    </p>
                </div>
            `).join('');
        }

        // Highlight node in graph
        function highlightNode(nodeId) {
            if (network) {
                network.selectNodes([nodeId]);
                network.focus(nodeId, { scale: 1.5, animation: true });
            }
        }

        // Show node detail panel
        function showNodeDetail(nodeId, nodeData) {
            const panel = document.getElementById('detail-panel');
            document.getElementById('detail-title').textContent = nodeData.label;
            document.getElementById('detail-content').innerHTML = `
                <p><strong>Type:</strong> ${nodeData.type}</p>
                <p><strong>Group:</strong> ${nodeData.group}</p>
                <p><strong>ID:</strong> ${nodeId}</p>
            `;
            panel.classList.add('active');
        }

        function closeDetail() {
            document.getElementById('detail-panel').classList.remove('active');
        }

        // Tab switching
        document.querySelectorAll('.tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
                tab.classList.add('active');
                loadList(tab.dataset.tab);
            });
        });

        // Search input
        let searchTimeout;
        document.getElementById('search').addEventListener('input', (e) => {
            clearTimeout(searchTimeout);
            searchTimeout = setTimeout(() => searchMemories(e.target.value), 300);
        });

        // Initialize
        loadStats();
        loadGraph();
        loadList('memories');

        // Auto-refresh stats every 5 seconds
        setInterval(loadStats, 5000);
    </script>
</body>
</html>
"#;
