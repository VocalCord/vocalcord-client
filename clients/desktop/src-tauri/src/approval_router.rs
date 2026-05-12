//! Routes inbound messages → agent actions based on current state.
//!
//! See the policy table in `plan/.../vocalcord-desktop.md`. Pure
//! decision logic — no IO — so the table can be exhaustively tested.

use crate::state::AgentStatus;

/// Whole-message (case-insensitive, trimmed) matchers that count as
/// approval. Extending these is a one-line change.
pub const APPROVE_WORDS: &[&str] = &[
    "approve",
    "allow",
    "yes",
    "y",
    "ok",
    "okay",
    "go",
    "go ahead",
    "do it",
    "proceed",
    "lgtm",
    "+1",
    "✅",
    "👍",
];

pub const DENY_WORDS: &[&str] = &[
    "deny",
    "no",
    "n",
    "nope",
    "stop",
    "cancel",
    "abort",
    "skip",
    "❌",
    "👎",
];

/// What `approval_router` decides should happen with an inbound
/// message body, given the current agent state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// No active session — start one with this message as the prompt.
    StartSession,
    /// Active session — feed via `say(message, interrupt=true)`.
    SayInterrupt,
    /// Resume a finished session with the message.
    ResumeSession,
    /// Resolve the pending tool approval as Approved.
    ApproveTool { approval_id: String },
    /// Resolve the pending tool approval as Denied.
    DenyTool { approval_id: String },
    /// Two-step: deny the tool, then say(interrupt=true) to feed
    /// follow-up context. Used when waiting for approval but the
    /// reply isn't a clear yes/no.
    DenyToolThenSay { approval_id: String },
    /// Resolve the pending question with this body as the answer.
    AnswerQuestion { approval_id: String },
}

/// Classify a body as approval, denial, or other.
fn classify(body: &str) -> Classification {
    let norm = body.trim().to_lowercase();
    if APPROVE_WORDS.iter().any(|w| *w == norm) {
        Classification::Approve
    } else if DENY_WORDS.iter().any(|w| *w == norm) {
        Classification::Deny
    } else {
        Classification::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classification {
    Approve,
    Deny,
    Other,
}

/// Decide which action to take given the agent state and the inbound
/// body text. Pure function — no IO.
pub fn decide(agent: &AgentStatus, body: &str) -> Action {
    match agent {
        AgentStatus::Stopped => Action::StartSession,
        AgentStatus::Running { .. } => Action::SayInterrupt,
        AgentStatus::WaitingForApproval { approval_id, .. } => match classify(body) {
            Classification::Approve => Action::ApproveTool {
                approval_id: approval_id.clone(),
            },
            Classification::Deny => Action::DenyTool {
                approval_id: approval_id.clone(),
            },
            Classification::Other => Action::DenyToolThenSay {
                approval_id: approval_id.clone(),
            },
        },
        AgentStatus::WaitingForAnswer { approval_id, .. } => Action::AnswerQuestion {
            approval_id: approval_id.clone(),
        },
        AgentStatus::Error { .. } => Action::ResumeSession,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting_approval() -> AgentStatus {
        AgentStatus::WaitingForApproval {
            approval_id: "ap1".into(),
            tool_name: "Edit".into(),
        }
    }
    fn waiting_answer() -> AgentStatus {
        AgentStatus::WaitingForAnswer {
            approval_id: "qu1".into(),
            tool_name: "AskUser".into(),
        }
    }
    fn running() -> AgentStatus {
        AgentStatus::Running {
            session_id: "s1".into(),
        }
    }

    #[test]
    fn stopped_starts_session() {
        assert_eq!(
            decide(&AgentStatus::Stopped, "hello"),
            Action::StartSession
        );
    }

    #[test]
    fn running_is_say_interrupt() {
        assert_eq!(decide(&running(), "anything"), Action::SayInterrupt);
    }

    #[test]
    fn approve_words_match() {
        for word in ["approve", "Allow", "  yes  ", "OK", "lgtm", "✅"] {
            assert_eq!(
                decide(&waiting_approval(), word),
                Action::ApproveTool {
                    approval_id: "ap1".into()
                },
                "case: {word:?}"
            );
        }
    }

    #[test]
    fn deny_words_match() {
        for word in ["deny", "No", "abort", "❌"] {
            assert_eq!(
                decide(&waiting_approval(), word),
                Action::DenyTool {
                    approval_id: "ap1".into()
                },
                "case: {word:?}"
            );
        }
    }

    #[test]
    fn ambiguous_during_approval_denies_then_says() {
        let a = decide(&waiting_approval(), "add error handling first");
        assert_eq!(
            a,
            Action::DenyToolThenSay {
                approval_id: "ap1".into()
            }
        );
    }

    #[test]
    fn answer_question_always_passes_through() {
        let a = decide(&waiting_answer(), "production cluster");
        assert_eq!(
            a,
            Action::AnswerQuestion {
                approval_id: "qu1".into()
            }
        );
        // Even a literal "yes" during a question is still an answer.
        let a = decide(&waiting_answer(), "yes");
        assert_eq!(
            a,
            Action::AnswerQuestion {
                approval_id: "qu1".into()
            }
        );
    }

    #[test]
    fn error_state_resumes() {
        let s = AgentStatus::Error {
            message: "boom".into(),
        };
        assert_eq!(decide(&s, "try again"), Action::ResumeSession);
    }
}
