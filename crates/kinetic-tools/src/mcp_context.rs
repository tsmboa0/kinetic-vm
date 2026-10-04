//! Empty stand-in for pinned MCP resource blocks.
//!
//! Sessions no longer load MCP resources. The prompt renderer stays so a
//! holder of an empty block list emits no section.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedResourceBlock {
    pub key: String,
    pub rendered: String,
}

#[must_use]
pub fn render_pinned_resources_section(blocks: &[PinnedResourceBlock]) -> String {
    if blocks.is_empty() {
        return String::new();
    }
    let mut body = String::new();
    for block in blocks {
        body.push_str(&block.rendered);
        body.push('\n');
    }
    format!("## Pinned MCP Resources\n\n{body}")
}
