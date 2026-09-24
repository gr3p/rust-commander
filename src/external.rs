use std::path::Path;

pub fn viewer_argv(configured: &str, path: &Path) -> Vec<String> {
    argv(configured, None, "xdg-open", path)
}

pub fn editor_argv(configured: &str, editor_env: Option<&str>, path: &Path) -> Vec<String> {
    argv(configured, editor_env, "xdg-open", path)
}

fn argv(configured: &str, env: Option<&str>, fallback: &str, path: &Path) -> Vec<String> {
    let chosen = if !configured.trim().is_empty() {
        configured
    } else if let Some(env) = env.filter(|s| !s.trim().is_empty()) {
        env
    } else {
        fallback
    };
    let mut parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        parts.push(fallback.to_string());
    }
    parts.push(path.display().to_string());
    parts
}
