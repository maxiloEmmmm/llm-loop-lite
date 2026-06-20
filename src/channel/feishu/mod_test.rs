use std::collections::HashMap;

use crate::message::{UserInputOption, UserInputQuestion, UserInputRequest};

use super::{FeishuReceiveIdType, build_user_input_status_card, feishu_user_receive_id_type};

/// 飞书 union_id 前缀应使用 union_id 发送类型。
#[test]
fn user_receive_id_type_uses_union_id_for_on_prefix() {
    assert_eq!(
        feishu_user_receive_id_type("on_0a5ab1e60ad574c2cad570b08a43a872"),
        FeishuReceiveIdType::UnionId
    );
}

/// 飞书 open_id 前缀应使用 open_id 发送类型。
#[test]
fn user_receive_id_type_uses_open_id_for_ou_prefix() {
    assert_eq!(
        feishu_user_receive_id_type("ou_0a5ab1e60ad574c2cad570b08a43a872"),
        FeishuReceiveIdType::OpenId
    );
}

/// 用户输入卡片应压缩已选问题，适用于避免禁用按钮占用纵向空间。
#[test]
fn user_input_status_card_hides_answered_question_buttons() {
    let request = UserInputRequest {
        questions: vec![UserInputQuestion {
            id: "weather_api".to_string(),
            header: "天气".to_string(),
            question: "选择天气 API".to_string(),
            options: vec![
                UserInputOption {
                    label: "FakeWeather".to_string(),
                    description: String::new(),
                },
                UserInputOption {
                    label: "FakeWeather CN".to_string(),
                    description: String::new(),
                },
            ],
        }],
        auto_resolution_ms: None,
    };
    let mut answers = HashMap::new();
    answers.insert(
        "weather_api".to_string(),
        vec!["FakeWeather CN".to_string()],
    );

    let card = build_user_input_status_card("request-1", &request, &answers, true);
    let elements = card
        .pointer("/body/elements")
        .and_then(serde_json::Value::as_array)
        .expect("卡片必须包含元素");
    let markdown = elements[1]["content"]
        .as_str()
        .expect("问题必须渲染为 markdown");

    assert_eq!(card["schema"], "2.0");
    assert_eq!(card["body"]["padding"], "2px 12px 2px 12px");
    assert_eq!(card["body"]["vertical_spacing"], "2px");
    assert!(markdown.contains("1. 天气 ✅"));
    assert!(markdown.contains("已选：`FakeWeather CN`"));
    assert!(markdown.contains("其他选项：FakeWeather"));
    assert_eq!(elements.len(), 2);
}

/// 用户输入卡片未选问题应使用小按钮，适用于压缩待确认卡片高度。
#[test]
fn user_input_status_card_uses_tiny_buttons_for_unanswered_question() {
    let request = UserInputRequest {
        questions: vec![UserInputQuestion {
            id: "format".to_string(),
            header: "格式".to_string(),
            question: "推送格式偏好？".to_string(),
            options: vec![
                UserInputOption {
                    label: "纯文字简报".to_string(),
                    description: String::new(),
                },
                UserInputOption {
                    label: "Markdown表格".to_string(),
                    description: String::new(),
                },
            ],
        }],
        auto_resolution_ms: None,
    };

    let card = build_user_input_status_card("request-1", &request, &HashMap::new(), false);
    let columns = card
        .pointer("/body/elements/2/columns")
        .and_then(serde_json::Value::as_array)
        .expect("未回答问题必须展示按钮");
    let first_button = &columns[0]["elements"][0];
    let second_button = &columns[1]["elements"][0];

    assert_eq!(card["schema"], "2.0");
    assert_eq!(first_button["size"], "tiny");
    assert_eq!(second_button["size"], "tiny");
    assert_eq!(
        first_button["behaviors"][0]["value"]["llm_loop"],
        "request_user_input"
    );
}
