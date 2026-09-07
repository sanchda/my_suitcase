//! Resolve the stable backlog tiers to a CLI and an optional concrete model.
use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    #[serde(alias = "anthropic")]
    Claude,
    #[serde(alias = "openai")]
    Codex,
}

impl Backend {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "claude" | "anthropic" => Ok(Self::Claude),
            "codex" | "openai" => Ok(Self::Codex),
            _ => Err(format!(
                "invalid backend '{s}': expected auto, claude, or codex"
            )),
        }
    }

    pub fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Auto | Self::Claude => "claude",
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Codex => Self::Claude,
            Self::Auto | Self::Claude => Self::Codex,
        }
    }
}

pub fn is_tier(model: &str) -> bool {
    crate::backlog::MODEL_TIERS.contains(&model)
}

pub fn valid_model(model: &str) -> bool {
    if model.starts_with('!') && model.contains(',') {
        return false;
    }
    let model = model.strip_prefix('!').unwrap_or(model);
    !model.is_empty()
        && !model.starts_with(['-', '!'])
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Recognize concrete model families. Custom deployment names remain ambiguous.
pub fn infer_model(model: &str) -> Option<Backend> {
    let model = model.strip_prefix('!').unwrap_or(model);
    let family = model.split('-').next().unwrap_or(model);
    if matches!(family, "gpt" | "chatgpt" | "codex" | "o1" | "o3" | "o4")
        || matches!(model, "astra" | "sol" | "terra" | "luna")
    {
        Some(Backend::Codex)
    } else if model.starts_with("claude-")
        || matches!(
            model.split('[').next().unwrap_or(model),
            "fable" | "opus" | "sonnet" | "haiku"
        )
    {
        Some(Backend::Claude)
    } else {
        None
    }
}

#[derive(Debug, PartialEq)]
pub struct Selection {
    pub backend: Backend,
    pub exclusive: bool,
    /// None lets Codex use its configured default; never send a Claude tier to it.
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub fn resolve(cfg: &Config, requested: &str) -> Selection {
    let selection = resolve_primary(cfg, requested);
    if !selection.exclusive && cfg.failover_from == Some(selection.backend) {
        counterpart(cfg, requested, &selection)
    } else {
        selection
    }
}

fn resolve_primary(cfg: &Config, requested: &str) -> Selection {
    let exclusive = requested.starts_with('!');
    let requested = requested.strip_prefix('!').unwrap_or(requested);
    let mapped = cfg.tier_models.get(requested).map(String::as_str);
    let candidate = mapped.unwrap_or(requested);
    let exclusive = exclusive || candidate.starts_with('!');
    let candidate = candidate.strip_prefix('!').unwrap_or(candidate);
    let default_name = cfg.model.strip_prefix('!').unwrap_or(&cfg.model);
    let default_model = cfg
        .tier_models
        .get(default_name)
        .map(String::as_str)
        .unwrap_or(&cfg.model);
    let inherit_exclusive = !exclusive && is_tier(candidate) && default_model.starts_with('!');
    let exclusive = exclusive || inherit_exclusive;
    let default_model = default_model.strip_prefix('!').unwrap_or(default_model);
    let candidate = if inherit_exclusive {
        default_model
    } else {
        candidate
    };
    let backend = if exclusive {
        infer_model(candidate).unwrap_or_else(|| {
            if cfg.backend == Backend::Auto {
                Backend::Claude
            } else {
                cfg.backend
            }
        })
    } else if cfg.backend == Backend::Auto {
        infer_model(if is_tier(candidate) {
            default_model
        } else {
            candidate
        })
        .unwrap_or(Backend::Claude)
    } else {
        cfg.backend
    };
    let model = if backend == Backend::Codex && is_tier(candidate) {
        (!is_tier(default_model)).then(|| expand_alias(default_model).to_string())
    } else {
        Some(expand_alias(candidate).to_string())
    };
    let requested = requested.to_ascii_lowercase();
    let effort = match cfg.effort.as_str() {
        "inherit" => None,
        "auto" => Some(
            if requested.contains("haiku") {
                "low"
            } else if ["opus", "fable", "astra", "sol"]
                .iter()
                .any(|name| requested.contains(name))
            {
                "high"
            } else {
                "medium"
            }
            .to_string(),
        ),
        // Codex calls its highest reasoning level xhigh.
        "max" if backend == Backend::Codex => Some("xhigh".into()),
        other => Some(other.to_string()),
    };
    Selection {
        backend,
        exclusive,
        model,
        effort,
    }
}

fn expand_alias(model: &str) -> &str {
    match model {
        "fable" => "claude-fable-5-1",
        "astra" => "gpt-6-astra",
        "sol" => "gpt-5.6-sol",
        "terra" => "gpt-5.6-terra",
        "luna" => "gpt-5.6-luna",
        _ => model,
    }
}

/// Capability pairings are routing defaults, not claims of model equivalence.
pub fn counterpart(cfg: &Config, requested: &str, source: &Selection) -> Selection {
    let backend = source.backend.opposite();
    let name = source.model.as_deref().unwrap_or(requested);
    let custom = cfg
        .failover_models
        .get(name)
        .or_else(|| cfg.failover_models.get(requested))
        .filter(|m| infer_model(m) == Some(backend));
    let model = custom
        .map(|m| expand_alias(m.strip_prefix('!').unwrap_or(m)).to_string())
        .unwrap_or_else(|| {
            let name = name.to_ascii_lowercase();
            let tier =
                if name.contains("fable") || name.contains("mythos") || name.contains("astra") {
                    0
                } else if name.contains("opus") || name.contains("sol") || name == "gpt-5.5" {
                    1
                } else if name.contains("haiku") || name.contains("luna") || name.contains("mini") {
                    3
                } else {
                    2
                };
            if backend == Backend::Codex {
                [
                    "gpt-6-astra",
                    "gpt-5.6-sol",
                    "gpt-5.6-terra",
                    "gpt-5.6-luna",
                ][tier]
            } else {
                ["claude-fable-5-1", "opus", "sonnet", "haiku"][tier]
            }
            .to_string()
        });
    let effort = source
        .effort
        .as_deref()
        .map(|effort| match (backend, effort) {
            (Backend::Codex, "max") => "xhigh".into(),
            (Backend::Claude, "xhigh") => "max".into(),
            _ => effort.to_string(),
        });
    Selection {
        backend,
        exclusive: custom.is_some_and(|m| m.starts_with('!')),
        model: Some(model),
        effort,
    }
}

/// Codex exec options. None bypasses the sandbox; Some names a sandbox mode.
/// Exec-only options precede `resume`, whose flag grammar is narrower.
pub fn codex_args(selection: &Selection, sandbox: Option<&str>, ephemeral: bool) -> Vec<String> {
    let mut args = vec!["exec".into(), "--json".into()];
    if let Some(sandbox) = sandbox {
        args.extend(["--sandbox".into(), sandbox.into()]);
        args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
    } else {
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
    }
    if ephemeral {
        args.push("--ephemeral".into());
    }
    if let Some(model) = &selection.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &selection.effort {
        args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
    }
    args
}

pub fn check_cost_budget(cfg: &Config, selection: &Selection) -> Result<(), String> {
    if selection.backend == Backend::Codex && (cfg.max_cost_usd > 0.0 || cfg.budget_usd > 0.0) {
        return Err("Codex exec does not report USD cost: use --max-duration/--max-iterations instead of max_cost_usd/budget_usd for Codex runs".into());
    }
    Ok(())
}

/// Keep raw CLI arguments from replacing an exclusive selection or re-enabling
/// Claude's overload fallback. Other flags retain their original order.
pub fn extra_args(cfg: &Config, selection: &Selection) -> Vec<String> {
    if !selection.exclusive {
        return cfg.extra_args.clone();
    }
    let mut args = cfg.extra_args.iter();
    let mut kept = Vec::new();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--model" | "-m" | "--fallback-model") {
            args.next();
        } else if !arg.starts_with("--model=")
            && !arg.starts_with("--fallback-model=")
            && !arg.starts_with("-m")
        {
            kept.push(arg.clone());
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_models_ignore_foreign_backend_and_cooldown_routing() {
        for (name, provider, concrete) in [
            ("!astra", Backend::Codex, "gpt-6-astra"),
            ("!fable", Backend::Claude, "claude-fable-5-1"),
            ("!claude-fable-5", Backend::Claude, "claude-fable-5"),
        ] {
            let cfg = Config {
                backend: provider.opposite(),
                failover_from: Some(provider),
                ..Config::default()
            };
            let selected = resolve(&cfg, name);
            assert!(selected.exclusive);
            assert_eq!(selected.backend, provider);
            assert_eq!(selected.model.as_deref(), Some(concrete));
        }
        let cfg = Config {
            model: "!astra".into(),
            ..Config::default()
        };
        assert_eq!(resolve(&cfg, "opus").model.as_deref(), Some("gpt-6-astra"));
        assert!(resolve(&cfg, "opus").exclusive);
        assert_eq!(
            resolve(&Config::default(), "fable").model.as_deref(),
            Some("claude-fable-5-1")
        );
        for bad in ["!", "!!astra", "!-model", "!two models", "!fable,sonnet"] {
            assert!(!valid_model(bad), "{bad}");
        }
    }

    #[test]
    fn default_pairs_and_aliases_route_both_ways() {
        let cfg = Config::default();
        for (claude, openai, alias) in [
            ("claude-fable-5-1", "gpt-6-astra", "astra"),
            ("opus", "gpt-5.6-sol", "sol"),
            ("sonnet", "gpt-5.6-terra", "terra"),
            ("haiku", "gpt-5.6-luna", "luna"),
        ] {
            assert_eq!(
                counterpart(&cfg, claude, &resolve(&cfg, claude))
                    .model
                    .as_deref(),
                Some(openai)
            );
            assert_eq!(
                counterpart(&cfg, openai, &resolve(&cfg, openai))
                    .model
                    .as_deref(),
                Some(claude)
            );
            assert_eq!(resolve(&cfg, alias).model.as_deref(), Some(openai));
            assert_eq!(resolve(&cfg, alias).backend, Backend::Codex);
        }
    }

    #[test]
    fn defaults_remain_claude_and_openai_tiers_keep_the_selected_model() {
        let mut cfg = Config::default();
        assert_eq!(resolve(&cfg, "opus").backend, Backend::Claude);
        cfg.model = "gpt-test".into();
        let high = resolve(&cfg, "opus");
        assert_eq!(high.backend, Backend::Codex);
        assert_eq!(high.model.as_deref(), Some("gpt-test"));
        assert_eq!(high.effort.as_deref(), Some("high"));
        cfg.model = "sonnet".into();
        cfg.backend = Backend::Codex;
        assert_eq!(resolve(&cfg, "haiku").model, None);
        assert_eq!(resolve(&cfg, "haiku").effort.as_deref(), Some("low"));
    }

    #[test]
    fn tier_mapping_can_select_backend_and_concrete_models() {
        let mut cfg = Config::default();
        cfg.tier_models.insert("sonnet".into(), "gpt-base".into());
        cfg.tier_models.insert("opus".into(), "gpt-large".into());
        assert_eq!(resolve(&cfg, "sonnet").model.as_deref(), Some("gpt-base"));
        assert_eq!(resolve(&cfg, "opus").model.as_deref(), Some("gpt-large"));
        assert_eq!(resolve(&cfg, "haiku").model.as_deref(), Some("gpt-base"));
        assert_eq!(resolve(&cfg, "haiku").backend, Backend::Codex);
        cfg.effort = "max".into();
        assert_eq!(resolve(&cfg, "opus").effort.as_deref(), Some("xhigh"));
        cfg.effort = "inherit".into();
        assert_eq!(resolve(&cfg, "opus").effort, None);
    }

    #[test]
    fn explicit_backend_supports_custom_model_ids() {
        let cfg = Config {
            backend: Backend::Codex,
            ..Config::default()
        };
        assert_eq!(resolve(&cfg, "custom-deployment").backend, Backend::Codex);
        assert_eq!(
            resolve(&cfg, "custom-deployment").model.as_deref(),
            Some("custom-deployment")
        );
        for model in ["gpt-5.4", "o3", "o4-mini", "codex-mini-latest"] {
            assert_eq!(resolve(&Config::default(), model).backend, Backend::Codex);
        }
    }
}
