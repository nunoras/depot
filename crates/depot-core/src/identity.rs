pub fn remote_identity(origin: &str) -> Option<String> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        return url_identity(trimmed);
    }
    scp_identity(trimmed)
}

fn scp_identity(origin: &str) -> Option<String> {
    let trimmed = origin.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let (head, tail) = trimmed.split_once(':')?;
    if head.is_empty()
        || head.contains('/')
        || head.contains('\\')
        || tail.is_empty()
        || tail.starts_with('/')
        || tail.starts_with('\\')
    {
        return None;
    }
    let host = head.rsplit('@').next().unwrap_or(head);
    let path = tail.trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path.to_lowercase()))
}

fn url_identity(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let rest = match rest.split_once('@') {
        Some((user, after)) if !user.contains('/') => after,
        _ => rest,
    };
    let (authority, path) = rest.split_once('/')?;
    let host = authority.split(':').next().unwrap_or(authority);
    let path = path.trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path.to_lowercase()))
}
