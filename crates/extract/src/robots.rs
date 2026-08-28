//! Minimal robots.txt handling: crawl-delay + Disallow for the most specific
//! matching user-agent group (falling back to `*`).

/// Parsed robots directives relevant to us.
#[derive(Debug, Clone, Default)]
pub struct RobotsInfo {
    pub crawl_delay_ms: Option<u64>,
    disallow: Vec<String>,
}

impl RobotsInfo {
    /// Everything allowed, no delay.
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Whether `path` may be fetched.
    pub fn is_allowed(&self, path: &str) -> bool {
        !self
            .disallow
            .iter()
            .any(|rule| !rule.is_empty() && path.starts_with(rule))
    }

    /// Parse robots.txt, selecting the group matching `ua` else `*`.
    pub fn parse(text: &str, ua: &str) -> Self {
        let ua = ua.to_ascii_lowercase();
        // Groups: (agents, disallow, crawl_delay_ms)
        struct Group {
            agents: Vec<String>,
            disallow: Vec<String>,
            crawl_delay_ms: Option<u64>,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut last_was_agent = false;

        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            match key.as_str() {
                "user-agent" => {
                    // Consecutive user-agent lines share one group; a user-agent
                    // after a directive begins a new group.
                    if last_was_agent && !groups.is_empty() {
                        groups
                            .last_mut()
                            .unwrap()
                            .agents
                            .push(value.to_ascii_lowercase());
                    } else {
                        groups.push(Group {
                            agents: vec![value.to_ascii_lowercase()],
                            disallow: Vec::new(),
                            crawl_delay_ms: None,
                        });
                    }
                    last_was_agent = true;
                }
                "disallow" => {
                    last_was_agent = false;
                    if let Some(g) = groups.last_mut() {
                        g.disallow.push(value);
                    }
                }
                "crawl-delay" => {
                    last_was_agent = false;
                    if let Some(g) = groups.last_mut() {
                        if let Ok(secs) = value.parse::<f64>() {
                            g.crawl_delay_ms = Some((secs * 1000.0) as u64);
                        }
                    }
                }
                _ => {}
            }
        }

        let matches = |g: &Group, want: &str| g.agents.iter().any(|a| a == want);
        let chosen = groups
            .iter()
            .find(|g| g.agents.iter().any(|a| ua.contains(a.as_str()) && a != "*"))
            .or_else(|| groups.iter().find(|g| matches(g, "*")));

        match chosen {
            Some(g) => RobotsInfo {
                crawl_delay_ms: g.crawl_delay_ms,
                disallow: g.disallow.clone(),
            },
            None => RobotsInfo::allow_all(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crawl_delay_and_disallow_for_star() {
        let txt = "User-agent: *\nDisallow: /private\nCrawl-delay: 5\n";
        let info = RobotsInfo::parse(txt, "RustSS/0.1");
        assert_eq!(info.crawl_delay_ms, Some(5000));
        assert!(!info.is_allowed("/private/x"));
        assert!(info.is_allowed("/public"));
    }

    #[test]
    fn ua_specific_group_wins() {
        let txt =
            "User-agent: *\nDisallow: /\n\nUser-agent: rustss\nDisallow: /nope\nCrawl-delay: 2\n";
        let info = RobotsInfo::parse(txt, "RustSS/0.1 (+url)");
        assert_eq!(info.crawl_delay_ms, Some(2000));
        assert!(info.is_allowed("/anything"));
        assert!(!info.is_allowed("/nope/page"));
    }

    #[test]
    fn empty_is_allow_all() {
        let info = RobotsInfo::parse("", "RustSS");
        assert!(info.is_allowed("/whatever"));
        assert_eq!(info.crawl_delay_ms, None);
    }
}
