mod deepseek;

use common::profiles::schema::ProfileDef;
use serde_json::Value;

use self::deepseek::DeepSeekProxyAdapter;

#[derive(Debug, Clone, Default)]
pub struct ProviderProxyContext {
    pub launch_id: Option<String>,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ProviderProxyAdapter {
    None,
    DeepSeek(DeepSeekProxyAdapter),
}

impl ProviderProxyAdapter {
    pub fn for_profile(profile: &ProfileDef, context: ProviderProxyContext) -> Self {
        match profile.provider.as_str() {
            "deepseek" => Self::DeepSeek(DeepSeekProxyAdapter::new(
                profile.id.clone(),
                profile.provider_settings.deepseek.clone(),
                context,
            )),
            _ => Self::None,
        }
    }

    pub fn prepare_chat_request(&mut self, original_request: &Value, chat_request: &mut Value) {
        match self {
            Self::None => {}
            Self::DeepSeek(adapter) => adapter.prepare_chat_request(original_request, chat_request),
        }
    }

    pub fn observe_chat_completion(&mut self, completion: &Value) {
        match self {
            Self::None => {}
            Self::DeepSeek(adapter) => adapter.observe_chat_completion(completion),
        }
    }

    pub fn observe_chat_stream_chunk(&mut self, chunk: &Value) {
        match self {
            Self::None => {}
            Self::DeepSeek(adapter) => adapter.observe_chat_stream_chunk(chunk),
        }
    }
}
