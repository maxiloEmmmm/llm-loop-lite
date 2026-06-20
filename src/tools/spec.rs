use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON Schema 基础类型，复制自 Codex tools 的最小可用子集。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaPrimitiveType {
    /// 字符串类型。
    String,
    /// 数字类型。
    Number,
    /// 布尔类型。
    Boolean,
    /// 整数类型。
    Integer,
    /// 对象类型。
    Object,
    /// 数组类型。
    Array,
    /// 空值类型。
    Null,
}

/// JSON Schema 的单类型或多类型声明。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum JsonSchemaType {
    /// 单个基础类型。
    Single(JsonSchemaPrimitiveType),
    /// 多个基础类型。
    Multiple(Vec<JsonSchemaPrimitiveType>),
}

/// JSON Schema 的 additionalProperties 表达。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AdditionalProperties {
    /// 是否允许额外字段。
    Boolean(bool),
    /// 额外字段的 schema。
    Schema(Box<JsonSchema>),
}

impl From<bool> for AdditionalProperties {
    /// 从布尔值构造 additionalProperties。
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

/// Responses tools 所需的 JSON Schema 子集。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JsonSchema {
    /// `$ref` 引用。
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    /// schema 类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<JsonSchemaType>,
    /// 字段说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 枚举值。
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<Value>>,
    /// 数组元素 schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    /// 对象字段 schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, JsonSchema>>,
    /// 必填字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// 是否允许额外字段。
    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<AdditionalProperties>,
    /// 任一 schema 匹配。
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<JsonSchema>>,
}

impl JsonSchema {
    /// 构造指定类型的 schema。
    fn typed(schema_type: JsonSchemaPrimitiveType, description: Option<String>) -> Self {
        Self {
            schema_type: Some(JsonSchemaType::Single(schema_type)),
            description,
            ..Default::default()
        }
    }

    /// 构造字符串 schema。
    pub fn string(description: Option<String>) -> Self {
        Self::typed(JsonSchemaPrimitiveType::String, description)
    }

    /// 构造数字 schema。
    pub fn number(description: Option<String>) -> Self {
        Self::typed(JsonSchemaPrimitiveType::Number, description)
    }

    /// 构造整数 schema。
    pub fn integer(description: Option<String>) -> Self {
        Self::typed(JsonSchemaPrimitiveType::Integer, description)
    }

    /// 构造布尔 schema。
    pub fn boolean(description: Option<String>) -> Self {
        Self::typed(JsonSchemaPrimitiveType::Boolean, description)
    }

    /// 构造 null schema。
    pub fn null(description: Option<String>) -> Self {
        Self::typed(JsonSchemaPrimitiveType::Null, description)
    }

    /// 构造字符串枚举 schema。
    pub fn string_enum(values: Vec<Value>, description: Option<String>) -> Self {
        Self {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::String)),
            description,
            enum_values: Some(values),
            ..Default::default()
        }
    }

    /// 构造数组 schema。
    pub fn array(items: JsonSchema, description: Option<String>) -> Self {
        Self {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array)),
            description,
            items: Some(Box::new(items)),
            ..Default::default()
        }
    }

    /// 构造对象 schema。
    pub fn object(
        properties: BTreeMap<String, JsonSchema>,
        required: Option<Vec<String>>,
        additional_properties: Option<AdditionalProperties>,
    ) -> Self {
        Self {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object)),
            properties: Some(properties),
            required,
            additional_properties,
            ..Default::default()
        }
    }

    /// 构造 anyOf schema。
    pub fn any_of(variants: Vec<JsonSchema>, description: Option<String>) -> Self {
        Self {
            description,
            any_of: Some(variants),
            ..Default::default()
        }
    }
}

/// Responses API function tool。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiTool {
    /// 工具名称。
    pub name: String,
    /// 工具描述。
    pub description: String,
    /// 是否启用严格 schema。
    pub strict: bool,
    /// 是否延迟加载。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    /// 输入参数 schema。
    pub parameters: JsonSchema,
    /// 输出 schema，目前仅保留给本地校验，不发给 Responses。
    #[serde(skip)]
    pub output_schema: Option<Value>,
}

/// Responses API freeform tool。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreeformTool {
    /// 工具名称。
    pub name: String,
    /// 工具描述。
    pub description: String,
    /// freeform 格式。
    pub format: FreeformToolFormat,
}

/// freeform tool 的格式定义。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreeformToolFormat {
    /// 格式类型。
    pub r#type: String,
    /// 语法名称。
    pub syntax: String,
    /// 语法定义。
    pub definition: String,
}

/// Responses API tool 规格，照 Codex wire shape 保持兼容。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSpec {
    /// JSON function tool。
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    /// 服务端图片生成 tool。
    #[serde(rename = "image_generation")]
    ImageGeneration {
        /// 输出格式。
        output_format: String,
    },
    /// 服务端 web search tool。
    #[serde(rename = "web_search")]
    WebSearch {
        /// 是否访问实时公网。
        #[serde(skip_serializing_if = "Option::is_none")]
        external_web_access: Option<bool>,
    },
    /// freeform custom tool。
    #[serde(rename = "custom")]
    Freeform(FreeformTool),
}

impl ToolSpec {
    /// 返回模型侧工具名称。
    pub fn name(&self) -> &str {
        match self {
            Self::Function(tool) => tool.name.as_str(),
            Self::ImageGeneration { .. } => "image_generation",
            Self::WebSearch { .. } => "web_search",
            Self::Freeform(tool) => tool.name.as_str(),
        }
    }
}

/// 把 tool specs 转成 Responses API `tools` JSON。
pub fn create_tools_json_for_responses_api(tools: &[ToolSpec]) -> serde_json::Result<Vec<Value>> {
    tools.iter().map(serde_json::to_value).collect()
}

/// 构造 hosted image generation tool。
pub fn create_image_generation_tool(output_format: &str) -> ToolSpec {
    ToolSpec::ImageGeneration {
        output_format: output_format.to_string(),
    }
}

/// 构造 hosted web search tool。
pub fn create_web_search_tool(live: bool) -> ToolSpec {
    ToolSpec::WebSearch {
        external_web_access: Some(live),
    }
}

/// 构造通用成功输出 JSON 字符串。
pub fn text_output(text: impl Into<String>, success: Option<bool>) -> Value {
    json!({
        "content": text.into(),
        "success": success,
    })
}
