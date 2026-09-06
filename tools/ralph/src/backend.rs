//! Resolve the stable backlog tiers to a CLI and an optional concrete model.
use crate::config::Config;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
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
}

pub fn is_tier(model: &str) -> bool {
    crate::backlog::MODEL_TIERS.contains(&model)
}

pub fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && !model.starts_with('-')
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Recognize concrete model families. Custom deployment names remain ambiguous.
pub fn infer_model(model: &str) -> Option<Backend> {
    let family = model.split('-').next().unwrap_or(model);
    if matches!(family, "gpt" | "chatgpt" | "codex" | "o1" | "o3" | "o4") {
        Some(Backend::Codex)
    } else if model.starts_with("claude-") {
        Some(Backend::Claude)
    } else {
        None
    }
}

#[derive(Debug, PartialEq)]
pub struct Selection {
    pub backend: Backend,
    /// None lets Codex use its configured default; never send a Claude tier to it.
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub fn resolve(cfg: &Config, requested: &str) -> Selection {
    let mapped = cfg.tier_models.get(requested).map(String::as_str);
    let candidate = mapped.unwrap_or(requested);
    let default_model = cfg.tier_models.get(&cfg.model).unwrap_or(&cfg.model);
    let backend = if cfg.backend == Backend::Auto {
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
        (!is_tier(default_model)).then(|| default_model.clone())
    } else {
        Some(candidate.to_string())
    };
    let requested = requested.to_ascii_lowercase();
    let effort = match cfg.effort.as_str() {
        "inherit" => None,
        "auto" => Some(
            if requested.contains("haiku") {
                "low"
            } else if requested.contains("opus") {
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
        model,
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

#[cfg(test)]
mod tests {
    use super::*;

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
