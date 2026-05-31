mod anthropic;
mod github;

pub use anthropic::{AnthropicPlugin, anthropic_auth_mode_get, anthropic_auth_mode_set};
pub use github::{GitHubPlugin, github_list_repos};
