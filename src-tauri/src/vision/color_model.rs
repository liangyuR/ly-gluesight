use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColorModelIssue {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColorModelStatus {
    pub model_present: bool,
    pub model_usable: bool,
    pub quality: Value,
    pub issues: Vec<ColorModelIssue>,
}

pub(crate) fn color_model_status(raw: Option<&str>) -> ColorModelStatus {
    let mut status = ColorModelStatus {
        model_present: raw.is_some(),
        model_usable: false,
        quality: Value::Null,
        issues: Vec::new(),
    };
    let issue = |code, message: &str| ColorModelIssue {
        code,
        message: message.into(),
    };
    let Some(raw) = raw else {
        status
            .issues
            .push(issue("modelMissing", "尚未保存颜色模型"));
        return status;
    };
    let Ok(model) = serde_json::from_str::<Value>(raw) else {
        status
            .issues
            .push(issue("modelInvalid", "颜色模型不是有效 JSON，请重新取色"));
        return status;
    };
    status.quality = model["quality"].clone();
    status.model_usable = status.quality["usable"].as_bool() == Some(true);
    if status.quality["glueCount"].as_u64().is_some_and(|n| n < 2) {
        status.issues.push(issue(
            "glueSamplesInsufficient",
            "胶色样本不足 2 个，请核对示教线与原图",
        ));
    }
    if status.quality["bgCount"].as_u64() == Some(0) {
        status.issues.push(issue(
            "backgroundSamplesMissing",
            "没有有效背景样本，请核对背景带、曝光与高光",
        ));
    }
    if model["background"].as_array().is_some_and(Vec::is_empty) {
        status
            .issues
            .push(issue("backgroundModelMissing", "没有可用的背景颜色分量"));
    }
    if status.quality["usable"].as_bool().is_none() {
        status
            .issues
            .push(issue("qualityUnknown", "模型没有可用性证据，请重新取色"));
    } else if !status.model_usable && status.issues.is_empty() {
        status.issues.push(issue(
            "modelUnusable",
            "取色质量标记为不可用，请核对样本数、分离度与原图",
        ));
    }
    if !status.issues.is_empty() {
        status.model_usable = false;
    }
    status
}
