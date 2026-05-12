//! Outbound replies. When the agent finishes a session, we send the
//! trailing output back as a reply on the channel the message arrived
//! on.

use vocalcord_mcp_client::{McpClient, McpError, SendMessageArgs};

use crate::settings::Settings;

/// Truncate `output` to at most `max_chars` characters, cutting at a
/// word boundary when possible and appending `…`.
pub fn truncate_for_reply(output: &str, max_chars: usize) -> String {
    let n = output.chars().count();
    if n <= max_chars {
        return output.to_owned();
    }
    let take = max_chars.saturating_sub(1);
    let prefix: String = output.chars().take(take).collect();
    // Cut at the last whitespace if there is one in the tail half.
    let cut = prefix
        .char_indices()
        .rev()
        .take(64)
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(prefix.len());
    let mut s = prefix[..cut].trim_end().to_string();
    s.push('…');
    s
}

/// Send the agent's reply for a given inbound message ID.
pub async fn send_reply(
    settings: &Settings,
    inbound_id: &str,
    body: String,
) -> Result<(), McpError> {
    let client = McpClient::new(settings.api_base.clone(), settings.api_key.clone());
    client
        .send_message(SendMessageArgs {
            body,
            reply_to_message_id: Some(inbound_id.to_string()),
            ..Default::default()
        })
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_outputs_pass_through() {
        let out = truncate_for_reply("hello world", 2048);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn long_outputs_truncate_with_ellipsis() {
        let big = "word ".repeat(1000);
        let out = truncate_for_reply(&big, 100);
        assert!(out.chars().count() <= 100);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncates_at_word_boundary_when_possible() {
        let s = "alpha beta gamma delta epsilon";
        let out = truncate_for_reply(s, 15);
        assert!(out.ends_with('…'));
        // Should not slice mid-word.
        let trimmed = out.trim_end_matches('…').trim_end();
        assert!(!trimmed.contains("delt"));
    }
}
