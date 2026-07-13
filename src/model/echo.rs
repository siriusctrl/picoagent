use anyhow::Result;
use async_trait::async_trait;

use crate::{
    events::{RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::{MessageContent, ModelProvider, ModelRequest, ModelResponse, ModelUsage},
};

pub struct EchoProvider;

#[async_trait]
impl ModelProvider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let prompt = request
            .messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter())
            .find_map(|content| match content {
                MessageContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        let text = format!("received: {prompt}");
        events
            .emit(&RuntimeEvent::new(
                request.run_id,
                RuntimeEventKind::ModelDelta { text: text.clone() },
            ))
            .await?;
        Ok(ModelResponse {
            text,
            tool_calls: Vec::new(),
            assistant_content: Vec::new(),
            usage: ModelUsage::default(),
        })
    }
}
