use super::{
    TelegramChat, TelegramMessage, TelegramPhotoSize, best_photo, build_message_reaction_body,
    build_user_input_keyboard, parse_user_input_callback, split_telegram_text, strip_bot_mention,
    telegram_source,
};
use crate::message::{UserInputOption, UserInputQuestion, UserInputRequest};

/// 构造 Telegram 测试消息，适用于来源映射用例。
fn test_message() -> TelegramMessage {
    TelegramMessage {
        message_id: 12,
        message_thread_id: Some(34),
        from: None,
        sender_chat: None,
        chat: TelegramChat {
            id: -100,
            kind: "supergroup".to_string(),
            _title: Some("群".to_string()),
            _username: None,
        },
        text: Some("hello".to_string()),
        caption: None,
        photo: None,
        document: None,
        audio: None,
        video: None,
        voice: None,
        animation: None,
    }
}

/// Telegram supergroup 会映射为统一 group 来源，并保留 topic id。
#[test]
fn telegram_source_maps_supergroup_and_thread() {
    let source = telegram_source("tg-main", &test_message());

    assert_eq!(source.channel_name, "tg-main");
    assert_eq!(source.platform, "telegram");
    assert_eq!(source.chat_id, "-100");
    assert_eq!(source.chat_type, "group");
    assert_eq!(source.thread_id.as_deref(), Some("34"));
}

/// require_mention 路径会移除 bot mention。
#[test]
fn strip_bot_mention_removes_username() {
    let text = "你好 @demo_bot 帮我查日志";

    let cleaned = strip_bot_mention(text, "demo_bot").expect("应命中 mention");

    assert_eq!(cleaned, "你好  帮我查日志");
}

/// 未 @ bot 时不会触发 require_mention 消息。
#[test]
fn strip_bot_mention_rejects_missing_username() {
    assert!(strip_bot_mention("你好", "demo_bot").is_none());
}

/// Telegram 文本会按字符上限分段。
#[test]
fn split_telegram_text_splits_long_message() {
    let text = format!("{}\n{}", "a".repeat(3900), "b".repeat(3900));

    let chunks = split_telegram_text(&text);

    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].chars().count() <= 3900);
    assert!(chunks[1].chars().count() <= 3900);
}

/// Telegram 图片选择最大尺寸。
#[test]
fn best_photo_uses_largest_area() {
    let photos = vec![
        TelegramPhotoSize {
            file_id: "small".to_string(),
            _file_unique_id: "s".to_string(),
            width: 10,
            height: 10,
            _file_size: None,
        },
        TelegramPhotoSize {
            file_id: "large".to_string(),
            _file_unique_id: "l".to_string(),
            width: 20,
            height: 20,
            _file_size: None,
        },
    ];

    let photo = best_photo(&photos).expect("应选择图片");

    assert_eq!(photo.file_id, "large");
}

/// Telegram 用户输入 callback data 可解析请求和选项下标。
#[test]
fn parse_user_input_callback_reads_indices() {
    let parsed = parse_user_input_callback("ui:req1:2:3").expect("应解析 callback");

    assert_eq!(parsed, ("req1", 2, 3));
}

/// Telegram inline keyboard 使用稳定 request id。
#[test]
fn build_user_input_keyboard_contains_callback_data() {
    let request = UserInputRequest {
        questions: vec![UserInputQuestion {
            id: "mode".to_string(),
            header: "模式".to_string(),
            question: "选哪个？".to_string(),
            options: vec![UserInputOption {
                label: "快".to_string(),
                description: "更快".to_string(),
            }],
        }],
        auto_resolution_ms: None,
    };

    let keyboard = build_user_input_keyboard("abc", &request);

    assert_eq!(
        keyboard["inline_keyboard"][0][0]["callback_data"],
        "ui:abc:0:0"
    );
}

/// Telegram reaction 请求体保持 Bot API 要求的 emoji reaction 结构。
#[test]
fn build_message_reaction_body_uses_emoji_reaction() {
    let body = build_message_reaction_body("1000000000", 2, "\u{1F440}", true);

    assert_eq!(body["chat_id"], "1000000000");
    assert_eq!(body["message_id"], 2);
    assert_eq!(body["reaction"][0]["type"], "emoji");
    assert_eq!(body["reaction"][0]["emoji"], "\u{1F440}");
    assert_eq!(body["is_big"], true);
}
