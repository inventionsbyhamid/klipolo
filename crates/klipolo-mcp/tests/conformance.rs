//! A real MCP client drives Klipolo's server: the official Rust SDK (`rmcp`) over Streamable HTTP finds it through the
//! connection file, opens a session, lists the tools, reads the context as a resource and as a tool, applies a format
//! and proposes a cut, then ends its session. The two-clip project is on a `FakeHost`.

#[path = "support/fake_host.rs"]
mod fake_host;

use std::sync::Arc;
use std::time::Duration;

use fake_host::FakeHost;
use klipolo_mcp::connection::{self, McpServer};
use klipolo_mcp::rpc::{Mcp, listed};
use klipolo_mcp::words::INSTRUCTIONS;
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation,
    ReadResourceRequestParams,
};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Map, Value, json};
use tokio::runtime::Handle;

fn arguments(args: Value) -> Map<String, Value> {
    args.as_object().expect("an object").clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rmcp_client_initializes_lists_tools_reads_and_calls() {
    let temp = tempfile::tempdir().unwrap();
    let host = FakeHost::two_clips();
    let mcp = Arc::new(Mcp::new(host.clone()));
    let server = McpServer::start(&Handle::current(), mcp, temp.path()).unwrap();
    // The client finds Klipolo as the bridge does, through the connection file.
    let file = connection::read(temp.path()).expect("a connection file");
    let url = format!("http://127.0.0.1:{}/mcp", file.port);
    let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(file.token);
    let transport = StreamableHttpClientTransport::from_config(config);
    let info = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("Conformance agent", "1.0"),
    );
    let client = info.serve(transport).await.expect("initialize");

    // initialize: Klipolo, at a protocol version it speaks (the client asks for a newer one).
    let peer = client.peer_info().expect("the server's answer");
    let peer = serde_json::to_value(&*peer).unwrap();
    assert_eq!(peer["serverInfo"]["name"], "klipolo", "{peer}");
    assert_eq!(peer["serverInfo"]["title"], "Klipolo");
    assert_eq!(peer["protocolVersion"], "2025-11-25");
    assert_eq!(peer["instructions"], INSTRUCTIONS);
    assert_eq!(server.connected(), ["Conformance agent"]);

    // tools/list: every tool Klipolo lists, the reads among them.
    let tools = client.list_all_tools().await.expect("tools/list");
    let expected = listed("tools/list").unwrap()["tools"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(tools.len(), expected);
    for read in ["getSummary", "getContext", "getTranscript", "getProposal"] {
        assert!(tools.iter().any(|t| t.name == read), "{read}");
    }

    // resources/read klipolo://context, and the same read as the tool getContext.
    let read = (client.read_resource(ReadResourceRequestParams::new("klipolo://context")))
        .await
        .expect("resources/read");
    let read = serde_json::to_value(&read).unwrap();
    let context = read["contents"][0]["text"]
        .as_str()
        .expect("a text")
        .to_owned();
    assert!(context.starts_with("Revision "), "{context}");
    let tool = (client.call_tool(CallToolRequestParams::new("getContext")))
        .await
        .expect("getContext");
    let tool = serde_json::to_value(&tool).unwrap();
    assert_eq!(tool["isError"], false, "{tool}");
    assert_eq!(tool["content"][0]["text"], context);

    // One setFormat, which applies at once as one step.
    let before = host.revision();
    let format = CallToolRequestParams::new("setFormat")
        .with_arguments(arguments(json!({"format": "square", "keepOthers": true})));
    let applied = client.call_tool(format).await.expect("setFormat");
    let applied = serde_json::to_value(&applied).unwrap();
    assert_eq!(applied["isError"], false, "{applied}");
    assert_eq!(applied["structuredContent"]["outcome"], "done");
    assert_eq!(host.revision(), before + 1);

    // One cutRange on what it read, which waits for the person: nothing is cut.
    (client.call_tool(CallToolRequestParams::new("getContext")))
        .await
        .expect("getContext");
    let revision = host.revision();
    let cut = CallToolRequestParams::new("cutRange")
        .with_arguments(arguments(json!({"startSec": 0.0, "endSec": 13.0})));
    let proposed = client.call_tool(cut).await.expect("cutRange");
    let proposed = serde_json::to_value(&proposed).unwrap();
    assert_eq!(proposed["isError"], false, "{proposed}");
    assert_eq!(proposed["structuredContent"]["outcome"], "proposed");
    assert_eq!(proposed["structuredContent"]["removedSec"], 13.0);
    assert_eq!(host.revision(), revision);
    let slot = host.state().slot.clone().expect("a proposal waits");
    assert_eq!(slot.from.as_deref(), Some("Conformance agent"));

    // Closing, the client ends its session.
    client.cancel().await.expect("the client closes");
    for _ in 0..40 {
        if server.connected().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(server.connected().is_empty(), "{:?}", server.connected());
    server.stop();
}
