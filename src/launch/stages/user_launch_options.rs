use crate::core::dll_overrides::{dll_override_entries, merge_dll_overrides};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind, PipelineContext, PipelineStage};
use crate::launch::runners::CommandSpec;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Steam's game placeholder in launch options.
pub const COMMAND_MARKER: &str = "%command%";

/// Merged per DLL, not replaced.
const DLL_OVERRIDES_VAR: &str = "WINEDLLOVERRIDES";

/// User launch options, split Steam-style.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Leading `VAR=value` assignments.
    pub env: Vec<(String, String)>,
    /// Wrapper argv, outermost first.
    pub wrappers: Vec<String>,
    /// Extra game arguments.
    pub args: Vec<String>,
}

impl LaunchOptions {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.wrappers.is_empty() && self.args.is_empty()
    }
}

/// Parse launch options like Steam.
pub fn parse_launch_options(raw: &str) -> Result<LaunchOptions, String> {
    let mut parsed = LaunchOptions::default();
    let Some((before, after)) = raw.split_once(COMMAND_MARKER) else {
        parsed.args = game_args(split_command_line(raw)?);
        return Ok(parsed);
    };
    if after.contains(COMMAND_MARKER) {
        return Err(format!("{COMMAND_MARKER} may appear only once"));
    }
    let mut prefix = split_command_line(before)?.into_iter().peekable();
    while let Some(assignment) = prefix.next_if(|t| env_assignment(t).is_some()) {
        if let Some((name, value)) = env_assignment(&assignment) {
            parsed.env.push((name.to_string(), value.to_string()));
        }
    }
    parsed.wrappers = prefix.collect();
    parsed.args = game_args(split_command_line(after)?);
    Ok(parsed)
}

/// Drop Aurelia's own MangoHud switch.
fn game_args(tokens: Vec<String>) -> Vec<String> {
    tokens
        .into_iter()
        .filter(|t| t != "-mangohud" && t != "--mangohud")
        .collect()
}

/// `NAME=value` with a shell-valid name.
fn env_assignment(token: &str) -> Option<(&str, &str)> {
    let (name, value) = token.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then_some((name, value))
}

/// Shell-quoted split, without expansion.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err("unterminated double quote".into()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                in_token = true;
                match chars.next() {
                    Some(c) => current.push(c),
                    None => return Err("trailing backslash".into()),
                }
            }
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Wrap the command; user env wins.
pub fn wrap_command(spec: &mut CommandSpec, options: &LaunchOptions) {
    for (name, value) in &options.env {
        let merged = match spec.env.get(name) {
            // User wins per DLL only.
            Some(computed) if name == DLL_OVERRIDES_VAR => {
                merge_dll_overrides(computed, &dll_override_entries(value))
            }
            _ => value.clone(),
        };
        spec.env.insert(name.clone(), merged);
    }
    spec.args.extend(options.args.iter().cloned());
    let Some((wrapper, wrapper_args)) = options.wrappers.split_first() else {
        return;
    };
    let game = std::mem::replace(&mut spec.program, PathBuf::from(wrapper));
    let mut args = wrapper_args.to_vec();
    args.push(game.to_string_lossy().into_owned());
    args.append(&mut spec.args);
    spec.args = args;
}

/// Find wrapper binary like the shell.
fn wrapper_exists(name: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    path_var.is_some_and(|paths| std::env::split_paths(paths).any(|dir| dir.join(name).is_file()))
}

/// Applies user launch options.
pub struct UserLaunchOptionsStage;

#[async_trait]
impl PipelineStage for UserLaunchOptionsStage {
    fn name(&self) -> &str {
        "UserLaunchOptions"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let Some(raw) = ctx
            .user_config
            .as_ref()
            .map(|c| c.launch_options.clone())
            .filter(|o| !o.trim().is_empty())
        else {
            return Ok(());
        };
        let options = parse_launch_options(&raw).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Validation,
                format!("invalid launch options {raw:?}: {e}"),
            )
        })?;
        let Some(spec) = ctx.command_spec.as_mut() else {
            return Ok(());
        };
        if let Some(wrapper) = options.wrappers.first() {
            let path = spec
                .env
                .get("PATH")
                .map(std::ffi::OsString::from)
                .or_else(|| std::env::var_os("PATH"));
            if !wrapper_exists(wrapper, path.as_deref()) {
                return Err(LaunchError::new(
                    LaunchErrorKind::Dependency,
                    format!("launch option wrapper `{wrapper}` was not found in PATH"),
                ));
            }
        }
        tracing::info!(
            env = options.env.len(),
            wrappers = ?options.wrappers,
            args = ?options.args,
            "applying launch options"
        );
        wrap_command(spec, &options);
        Ok(())
    }
}

#[cfg(test)]
#[path = "user_launch_options_tests.rs"]
mod tests;
