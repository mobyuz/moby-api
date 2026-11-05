use std::fmt;

pub(crate) const DEFAULT_MODEL: &str = "anthropic/claude-haiku-5.5";
pub(crate) const DEFAULT_FALLBACK: &str = "openai/gpt-6-luna";
const MAX_FALLBACKS: usize = 3;
// 100 USD per million tokens. A larger price is more likely a unit mistake.
const MAX_PRICE: u64 = 100_000_000;

pub(crate) type Lookup<'a> = &'a dyn Fn(&'static str) -> Result<Option<String>, &'static str>;

/// An API key that never appears in `Debug` output.
#[derive(Clone)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}

/// Integer microUSD per one million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Price {
    pub(crate) input: u64,
    pub(crate) output: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenRouterConfig {
    pub(crate) key: Secret,
    // The primary model comes first. OpenRouter tries the rest in order.
    pub(crate) models: Vec<String>,
    pub(crate) prices: Vec<(String, Price)>,
    pub(crate) zdr: bool,
    pub(crate) agent_max_output_tokens: u32,
    pub(crate) demo_max_output_tokens: u32,
}

impl OpenRouterConfig {
    pub(crate) fn from_lookup(get: Lookup) -> Result<Self, &'static str> {
        let key = get("MOBY_OPENROUTER_API_KEY")?
            .filter(|key| !key.is_empty())
            .ok_or("MOBY_OPENROUTER_API_KEY is required for openrouter")?;
        if key.len() > 512 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("MOBY_OPENROUTER_API_KEY must be 1-512 printable ASCII characters");
        }

        let primary = get("MOBY_OPENROUTER_MODEL")?.unwrap_or_else(|| DEFAULT_MODEL.into());
        let fallbacks = match get("MOBY_OPENROUTER_FALLBACK_MODELS")? {
            Some(list) => list
                .split(',')
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_owned)
                .collect(),
            None => vec![DEFAULT_FALLBACK.to_owned()],
        };
        let mut models = vec![primary];
        models.extend(fallbacks);
        if models.len() > 1 + MAX_FALLBACKS {
            return Err("MOBY_OPENROUTER_FALLBACK_MODELS accepts at most 3 models");
        }
        for (index, model) in models.iter().enumerate() {
            if !valid_model(model) || models[..index].contains(model) {
                return Err("OpenRouter models must be unique slugs such as vendor/model");
            }
        }

        let prices = parse_prices(&get("MOBY_OPENROUTER_PRICES")?.unwrap_or_default())?;
        if prices.len() != models.len()
            || !models
                .iter()
                .all(|model| prices.iter().any(|(priced, _)| priced == model))
        {
            return Err("MOBY_OPENROUTER_PRICES must price every configured model exactly once");
        }

        Ok(Self {
            key: Secret(key),
            models,
            prices,
            zdr: get("MOBY_OPENROUTER_ZDR")?
                .as_deref()
                .unwrap_or("true")
                .parse()
                .map_err(|_| "MOBY_OPENROUTER_ZDR must be true or false")?,
            agent_max_output_tokens: output_tokens(
                get,
                "MOBY_OPENROUTER_MAX_OUTPUT_TOKENS",
                300,
                1024,
            )?,
            demo_max_output_tokens: output_tokens(
                get,
                "MOBY_DEMO_OPENROUTER_MAX_OUTPUT_TOKENS",
                120,
                512,
            )?,
        })
    }

    pub(crate) fn price(&self, model: &str) -> Option<Price> {
        self.prices
            .iter()
            .find(|(priced, _)| priced == model)
            .map(|(_, price)| *price)
    }
}

fn output_tokens(
    get: Lookup,
    name: &'static str,
    default: u32,
    max: u32,
) -> Result<u32, &'static str> {
    // OpenRouter documents that some providers enforce a minimum of 16.
    match get(name)? {
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|tokens| (16..=max).contains(tokens))
            .ok_or(name),
        None => Ok(default),
    }
}

fn valid_model(model: &str) -> bool {
    (1..=128).contains(&model.len())
        && !model.starts_with('/')
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-:~".contains(&byte))
}

// Entries look like `vendor/model=100000:500000`: input and output microUSD
// per million tokens. A model slug can contain `:` but never `=`.
fn parse_prices(raw: &str) -> Result<Vec<(String, Price)>, &'static str> {
    const INVALID: &str = "MOBY_OPENROUTER_PRICES must look like vendor/model=input:output with microUSD per 1M tokens";
    let mut prices: Vec<(String, Price)> = Vec::new();
    for entry in raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (model, amounts) = entry.rsplit_once('=').ok_or(INVALID)?;
        let (input, output) = amounts.split_once(':').ok_or(INVALID)?;
        let amount = |text: &str| {
            text.parse::<u64>()
                .ok()
                // Zero would let a paid call reserve no cost and bypass a zero budget.
                .filter(|price| (1..=MAX_PRICE).contains(price))
                .ok_or(INVALID)
        };
        if !valid_model(model) || prices.iter().any(|(priced, _)| priced == model) {
            return Err(INVALID);
        }
        prices.push((
            model.to_owned(),
            Price {
                input: amount(input)?,
                output: amount(output)?,
            },
        ));
    }
    Ok(prices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "synthetic-openrouter-key";
    const PRICES: &str = "anthropic/claude-haiku-5.5=100000:500000,openai/gpt-6-luna=100000:500000";

    fn config(vars: &[(&str, &str)]) -> Result<OpenRouterConfig, &'static str> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect();
        OpenRouterConfig::from_lookup(&|name| {
            Ok(vars
                .iter()
                .find(|(var, _)| var == name)
                .map(|(_, value)| value.clone()))
        })
    }

    #[test]
    fn defaults_need_a_key_and_prices() {
        assert!(config(&[]).is_err());
        assert!(config(&[("MOBY_OPENROUTER_API_KEY", KEY)]).is_err());
        let config = config(&[
            ("MOBY_OPENROUTER_API_KEY", KEY),
            ("MOBY_OPENROUTER_PRICES", PRICES),
        ])
        .unwrap();
        assert_eq!(config.models, [DEFAULT_MODEL, DEFAULT_FALLBACK]);
        assert!(config.zdr);
        assert_eq!(
            (
                config.agent_max_output_tokens,
                config.demo_max_output_tokens
            ),
            (300, 120)
        );
        assert_eq!(
            config.price(DEFAULT_MODEL),
            Some(Price {
                input: 100_000,
                output: 500_000
            })
        );
    }

    #[test]
    fn rejects_bad_values_without_echoing_them() {
        let base = [
            ("MOBY_OPENROUTER_API_KEY", KEY),
            ("MOBY_OPENROUTER_PRICES", PRICES),
        ];
        let with = |name: &'static str, value: &'static str| {
            let mut vars = base.to_vec();
            vars.retain(|(var, _)| *var != name);
            vars.push((name, value));
            config(&vars)
        };
        for (name, value) in [
            ("MOBY_OPENROUTER_API_KEY", ""),
            ("MOBY_OPENROUTER_API_KEY", "has space"),
            ("MOBY_OPENROUTER_MODEL", "bad model"),
            ("MOBY_OPENROUTER_MODEL", "openai/gpt-6-luna"),
            ("MOBY_OPENROUTER_FALLBACK_MODELS", "a/b,c/d,e/f,g/h"),
            (
                "MOBY_OPENROUTER_PRICES",
                "anthropic/claude-haiku-5.5=100000:500000",
            ),
            (
                "MOBY_OPENROUTER_PRICES",
                "anthropic/claude-haiku-5.5=0:500000,openai/gpt-6-luna=1:1",
            ),
            (
                "MOBY_OPENROUTER_PRICES",
                "anthropic/claude-haiku-5.5=1:2,openai/gpt-6-luna=1:2,x/y=1:2",
            ),
            (
                "MOBY_OPENROUTER_PRICES",
                "anthropic/claude-haiku-5.5=1:2,anthropic/claude-haiku-5.5=1:2",
            ),
            ("MOBY_OPENROUTER_ZDR", "maybe"),
            ("MOBY_OPENROUTER_MAX_OUTPUT_TOKENS", "8"),
            ("MOBY_OPENROUTER_MAX_OUTPUT_TOKENS", "4096"),
            ("MOBY_DEMO_OPENROUTER_MAX_OUTPUT_TOKENS", "513"),
        ] {
            let error = with(name, value).unwrap_err();
            assert!(!error.contains(KEY), "{name} error leaked a value");
        }
        let custom = with("MOBY_OPENROUTER_FALLBACK_MODELS", "").unwrap_err();
        assert!(
            custom.contains("PRICES"),
            "a missing fallback price is caught"
        );
    }

    #[test]
    fn debug_output_redacts_the_key() {
        let config = config(&[
            ("MOBY_OPENROUTER_API_KEY", KEY),
            ("MOBY_OPENROUTER_PRICES", PRICES),
        ])
        .unwrap();
        let debug = format!("{config:?} {:#?}", config.key);
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains(KEY));
    }
}
