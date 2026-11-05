use std::sync::Arc;

use moby_protocol::AgentTurnRequest;

use super::{
    LlmProvider, MockProvider, ProviderStream,
    openrouter::{Lookup, OpenRouter, OpenRouterConfig},
};
use crate::usage::{Charge, Route};

/// The configured model provider. Cloning shares the same provider.
#[derive(Clone)]
pub struct Llm(Arc<dyn LlmProvider + Send + Sync>);

impl Llm {
    /// The deterministic provider with zero cost. It is the default.
    pub fn mock() -> Self {
        Self(Arc::new(MockProvider))
    }

    /// Whether a turn can spend money.
    pub fn is_paid(&self) -> bool {
        self.0.route().provider != "mock"
    }

    fn new(provider: impl LlmProvider + Send + Sync + 'static) -> Self {
        Self(Arc::new(provider))
    }
}

impl LlmProvider for Llm {
    fn estimate(&self, request: &AgentTurnRequest, prompt: &str) -> Charge {
        self.0.estimate(request, prompt)
    }

    fn stream(&self, request: Arc<AgentTurnRequest>, prompt: &'static str) -> ProviderStream {
        self.0.stream(request, prompt)
    }

    fn route(&self) -> Route {
        self.0.route()
    }
}

/// The providers for the agent route and the guest demo. Each is mock unless
/// its own variable selects `openrouter`.
pub(crate) struct Providers {
    pub(crate) agent: Llm,
    pub(crate) demo: Llm,
}

impl Providers {
    pub(crate) fn from_lookup(get: Lookup) -> Result<Self, &'static str> {
        let agent = selects_openrouter(get, "MOBY_LLM_PROVIDER")?;
        let demo = selects_openrouter(get, "MOBY_DEMO_LLM_PROVIDER")?;
        if !agent && !demo {
            return Ok(Self {
                agent: Llm::mock(),
                demo: Llm::mock(),
            });
        }
        let config = OpenRouterConfig::from_lookup(get)?;
        let (agent_tokens, demo_tokens) = (
            config.agent_max_output_tokens,
            config.demo_max_output_tokens,
        );
        let openrouter = OpenRouter::new(config, agent_tokens)?;
        let pick = |selected: bool, tokens: u32| {
            if selected {
                Llm::new(openrouter.with_max_output_tokens(tokens))
            } else {
                Llm::mock()
            }
        };
        Ok(Self {
            agent: pick(agent, agent_tokens),
            demo: pick(demo, demo_tokens),
        })
    }
}

fn selects_openrouter(get: Lookup, name: &'static str) -> Result<bool, &'static str> {
    match get(name)?.as_deref() {
        None | Some("mock") => Ok(false),
        Some("openrouter") => Ok(true),
        Some(_) => Err(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn providers(vars: &[(&str, &str)]) -> Result<Providers, &'static str> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect();
        Providers::from_lookup(&|name| {
            Ok(vars
                .iter()
                .find(|(var, _)| var == name)
                .map(|(_, value)| value.clone()))
        })
    }

    #[test]
    fn mock_is_the_default_and_needs_no_openrouter_settings() {
        let providers = providers(&[]).unwrap();
        assert!(!providers.agent.is_paid() && !providers.demo.is_paid());
        let providers = self::providers(&[("MOBY_LLM_PROVIDER", "mock")]).unwrap();
        assert!(!providers.agent.is_paid());
    }

    #[test]
    fn selecting_openrouter_validates_at_startup() {
        for vars in [
            &[("MOBY_LLM_PROVIDER", "openrouter")][..],
            &[("MOBY_DEMO_LLM_PROVIDER", "openrouter")],
            &[
                ("MOBY_LLM_PROVIDER", "openrouter"),
                ("MOBY_OPENROUTER_API_KEY", "synthetic-key"),
            ],
            &[("MOBY_LLM_PROVIDER", "OpenRouter")],
            &[("MOBY_LLM_PROVIDER", "other")],
        ] {
            assert!(providers(vars).is_err(), "{vars:?}");
        }
        let ready = [
            ("MOBY_OPENROUTER_API_KEY", "synthetic-key"),
            (
                "MOBY_OPENROUTER_PRICES",
                "anthropic/claude-haiku-5.5=100000:500000,openai/gpt-6-luna=100000:500000",
            ),
        ];
        let mut agent_only = ready.to_vec();
        agent_only.push(("MOBY_LLM_PROVIDER", "openrouter"));
        let providers = self::providers(&agent_only).unwrap();
        assert!(providers.agent.is_paid() && !providers.demo.is_paid());
        let mut demo_only = ready.to_vec();
        demo_only.push(("MOBY_DEMO_LLM_PROVIDER", "openrouter"));
        let providers = self::providers(&demo_only).unwrap();
        assert!(!providers.agent.is_paid() && providers.demo.is_paid());
    }
}
