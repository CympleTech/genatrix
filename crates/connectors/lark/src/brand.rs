//! Feishu and Lark: one interface, two sets of hosts.

use serde::{Deserialize, Serialize};

/// Which of the two an account lives on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Brand {
    /// Lark, the international one.
    #[default]
    Lark,
    /// Feishu, in China.
    Feishu,
}

impl Brand {
    /// The open platform: the API, and the token endpoint.
    #[must_use]
    pub const fn open_host(self) -> &'static str {
        match self {
            Self::Lark => "open.larksuite.com",
            Self::Feishu => "open.feishu.cn",
        }
    }

    /// The accounts host, which only signing in needs.
    #[must_use]
    pub const fn accounts_host(self) -> &'static str {
        match self {
            Self::Lark => "accounts.larksuite.com",
            Self::Feishu => "accounts.feishu.cn",
        }
    }

    /// The API's base address.
    #[must_use]
    pub fn api(self) -> String {
        format!("https://{}/open-apis", self.open_host())
    }

    /// Its name, as stored.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lark => "lark",
            Self::Feishu => "feishu",
        }
    }
}

impl std::str::FromStr for Brand {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "lark" | "larksuite" => Ok(Self::Lark),
            "feishu" => Ok(Self::Feishu),
            other => Err(format!("{other} is neither lark nor feishu")),
        }
    }
}
