pub mod delivery;
pub mod discord;
pub mod feishu;
pub mod slack;
pub mod telegram;

// Re-export adapter types
pub use discord::DiscordAdapter;
pub use feishu::FeishuAdapter;
pub use slack::SlackAdapter;
pub use telegram::TelegramAdapter;
