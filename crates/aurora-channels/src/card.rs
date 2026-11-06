//! Card schema (docs/13 §4.3). Builders sanitize; [`Card::validate`] enforces
//! the limits for cards that arrive from elsewhere.

use aurora_core::sanitize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_FIELDS: usize = 12;
pub const MAX_BUTTONS: usize = 5;
pub const MAX_OPTIONS: usize = 25;
pub const MAX_BLOCKS: usize = 50;
pub const MAX_TOKEN_CHARS: usize = 2048;
const TEXT_MAX: usize = 3000;
const LABEL_MAX: usize = 80;
const URL_MAX: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CardError {
    #[error("card has too many blocks (max {MAX_BLOCKS})")]
    TooManyBlocks,
    #[error("too many {what} (max {max})")]
    TooMany { what: &'static str, max: usize },
    #[error("{what} must not be empty")]
    Empty { what: &'static str },
    #[error("{what} is too long (max {max})")]
    TooLong { what: &'static str, max: usize },
    #[error("link url must be absolute http(s)")]
    BadUrl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Danger,
    #[default]
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Field {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ButtonItem {
    pub label: String,
    #[serde(default)]
    pub style: ButtonStyle,
    /// Opaque action token; connectors never interpret it.
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Header {
        text: String,
    },
    Text {
        text: String,
    },
    Fields {
        pairs: Vec<Field>,
    },
    Context {
        text: String,
    },
    Link {
        label: String,
        url: String,
    },
    Buttons {
        items: Vec<ButtonItem>,
    },
    Select {
        placeholder: String,
        options: Vec<SelectOption>,
        token: String,
    },
    Input {
        label: String,
        token: String,
    },
}

impl Block {
    /// Core-authored header (platform voice, single line).
    pub fn header(text: &str) -> Self {
        Block::Header {
            text: sanitize::one_line(text, LABEL_MAX),
        }
    }

    /// Core-authored text block.
    pub fn text(text: &str) -> Self {
        Block::Text {
            text: sanitize::block(text, TEXT_MAX),
        }
    }

    /// Untrusted text wrapped as inert data (summaries, names, model output).
    pub fn data(raw: &str) -> Self {
        Block::Text {
            text: sanitize::markdown_as_data(raw, TEXT_MAX),
        }
    }

    pub fn context(text: &str) -> Self {
        Block::Context {
            text: sanitize::one_line(text, TEXT_MAX),
        }
    }

    /// Fields; values are untrusted by default and escaped as data.
    pub fn fields<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, CardError> {
        let pairs: Vec<Field> = pairs
            .into_iter()
            .map(|(l, v)| Field {
                label: sanitize::label(l),
                value: sanitize::markdown_as_data(v, 500),
            })
            .collect();
        check_len("fields", pairs.len(), MAX_FIELDS)?;
        Ok(Block::Fields { pairs })
    }

    pub fn link(label: &str, url: &str) -> Result<Self, CardError> {
        let url = url.trim();
        check_url(url)?;
        Ok(Block::Link {
            label: sanitize::label(label),
            url: url.to_owned(),
        })
    }

    pub fn buttons(items: Vec<ButtonItem>) -> Result<Self, CardError> {
        check_len("buttons", items.len(), MAX_BUTTONS)?;
        Ok(Block::Buttons { items })
    }

    pub fn select(
        placeholder: &str,
        options: impl IntoIterator<Item = (String, String)>,
        token: String,
    ) -> Result<Self, CardError> {
        let options: Vec<SelectOption> = options
            .into_iter()
            .map(|(l, v)| SelectOption {
                label: sanitize::label(&l),
                value: v,
            })
            .collect();
        check_len("select options", options.len(), MAX_OPTIONS)?;
        Ok(Block::Select {
            placeholder: sanitize::label(placeholder),
            options,
            token,
        })
    }

    pub fn input(label: &str, token: String) -> Self {
        Block::Input {
            label: sanitize::label(label),
            token,
        }
    }

    fn validate(&self) -> Result<(), CardError> {
        match self {
            Block::Fields { pairs } => check_len("fields", pairs.len(), MAX_FIELDS),
            Block::Link { url, .. } => check_url(url),
            Block::Buttons { items } => {
                check_len("buttons", items.len(), MAX_BUTTONS)?;
                items.iter().try_for_each(|b| check_token(&b.token))
            }
            Block::Select { options, token, .. } => {
                check_len("select options", options.len(), MAX_OPTIONS)?;
                check_token(token)
            }
            Block::Input { token, .. } => check_token(token),
            Block::Header { .. } | Block::Text { .. } | Block::Context { .. } => Ok(()),
        }
    }
}

impl ButtonItem {
    pub fn new(label: &str, style: ButtonStyle, token: String) -> Self {
        Self {
            label: sanitize::label(label),
            style,
            token,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Card {
    pub blocks: Vec<Block>,
}

impl Card {
    pub fn new(blocks: Vec<Block>) -> Result<Self, CardError> {
        let c = Self { blocks };
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), CardError> {
        if self.blocks.len() > MAX_BLOCKS {
            return Err(CardError::TooManyBlocks);
        }
        self.blocks.iter().try_for_each(Block::validate)
    }
}

fn check_len(what: &'static str, n: usize, max: usize) -> Result<(), CardError> {
    if n > max {
        Err(CardError::TooMany { what, max })
    } else {
        Ok(())
    }
}

fn check_token(t: &str) -> Result<(), CardError> {
    if t.is_empty() {
        Err(CardError::Empty { what: "token" })
    } else if t.chars().count() > MAX_TOKEN_CHARS {
        Err(CardError::TooLong {
            what: "token",
            max: MAX_TOKEN_CHARS,
        })
    } else {
        Ok(())
    }
}

fn check_url(url: &str) -> Result<(), CardError> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let ok = rest.is_some_and(|r| !r.is_empty())
        && url.len() <= URL_MAX
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok { Ok(()) } else { Err(CardError::BadUrl) }
}
