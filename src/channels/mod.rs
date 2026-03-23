pub mod delivery;

#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "feishu")]
pub mod feishu;
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "telegram")]
pub mod telegram;
#[cfg(feature = "weixin")]
pub mod weixin;

// Re-export adapter types
#[cfg(feature = "discord")]
pub use discord::DiscordAdapter;
#[cfg(feature = "feishu")]
pub use feishu::FeishuAdapter;
#[cfg(feature = "slack")]
pub use slack::SlackAdapter;
#[cfg(feature = "telegram")]
pub use telegram::TelegramAdapter;
#[cfg(feature = "weixin")]
pub use weixin::WeixinAdapter;
