pub mod delivery;

#[cfg(feature = "telegram")]
pub mod telegram;
#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "slack")]
pub mod slack;
#[cfg(feature = "feishu")]
pub mod feishu;

// Re-export adapter types
#[cfg(feature = "telegram")]
pub use telegram::TelegramAdapter;
#[cfg(feature = "discord")]
pub use discord::DiscordAdapter;
#[cfg(feature = "slack")]
pub use slack::SlackAdapter;
#[cfg(feature = "feishu")]
pub use feishu::FeishuAdapter;
