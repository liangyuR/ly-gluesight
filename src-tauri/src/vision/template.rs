//! 测量图模板（LF-4）：GraphDoc 加顶层图参数，随程序版本发布。每次运行由 GlueSight 合成全部有效参数，经 params_json 传给 lyFlow；
//! 模板里标 `advanced` 的是专家参数（LF-5），配方可以按相机覆盖，其余由 GlueSight 按业务参数换算，不对外开放。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// 一张测量图模板：GraphDoc 原文与解析出的顶层图参数规格。
pub struct Template {
    pub id: &'static str,
    pub text: &'static str,
    pub doc: Value,
    specs: Map<String, Value>,
}

/// 配方里的专家参数（模板参数名 → 值）。
pub type ExpertParams = BTreeMap<String, Value>;

fn load(id: &'static str, text: &'static str) -> Template {
    let doc: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("测量图模板 {id} 不是合法 JSON：{e}"));
    let specs = doc["params"].as_object().cloned().unwrap_or_else(|| panic!("测量图模板 {id} 没有顶层图参数"));
    Template { id, text, doc, specs }
}

fn templates() -> &'static [Template; 2] {
    static ALL: OnceLock<[Template; 2]> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            load("corridor", include_str!("../../graphs/corridor.lyflow.json")),
            load("heading", include_str!("../../graphs/heading.lyflow.json")),
        ]
    })
}

/// 示教走廊测量图（`glue.taught_path` → `glue.bead_width`，可接找边）。
pub fn corridor() -> &'static Template {
    &templates()[0]
}

/// 逐帧定向测量图（`glue.bead_path` → `glue.bead_width`，可接找边）；搜索与回退共用，按 `headingSource` 区分。
pub fn heading() -> &'static Template {
    &templates()[1]
}

pub fn all() -> impl Iterator<Item = &'static Template> {
    templates().iter()
}

fn is_expert(spec: &Value) -> bool {
    spec["advanced"] == true
}

/// 各模板的专家参数规格；同名参数在不同模板里绑定不同节点，但规格（范围、默认值、说明）必须一致。
pub fn expert_specs() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for t in all() {
        for (name, spec) in &t.specs {
            if !is_expert(spec) {
                continue;
            }
            let mut spec = spec.clone();
            spec.as_object_mut().map(|s| s.remove("binds"));
            if let Some(other) = out.get(name) {
                assert_eq!(other, &spec, "测量图模板的专家参数 {name} 在各模板里规格不一致");
            }
            out.insert(name.clone(), spec);
        }
    }
    out
}

/// 专家参数合法：名字是模板里的专家参数，值的类型与范围符合规格。`what` 是出错时的主语。
pub fn validate_expert(params: &ExpertParams, what: &str) -> Result<(), String> {
    let specs = expert_specs();
    for (name, value) in params {
        let spec = specs.get(name).ok_or_else(|| format!("{what}：没有名为 {name} 的专家参数"))?;
        let label = spec["label"].as_str().unwrap_or(name);
        let number = value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{what}：专家参数「{label}」需为有限数"))?;
        let (lo, hi) = (spec["min"].as_f64().unwrap_or(f64::NEG_INFINITY), spec["max"].as_f64().unwrap_or(f64::INFINITY));
        if !(lo..=hi).contains(&number) {
            return Err(format!("{what}：专家参数「{label}」{number} 需在 {lo}–{hi} 之间"));
        }
    }
    Ok(())
}

impl Template {
    /// 这次运行的全部有效参数：模板默认值，叠上 GlueSight 换算的业务参数（`business`，必须是非专家参数），
    /// 再按顺序叠上各层专家参数（配方默认、相机覆盖；只取本模板有的专家参数）。
    pub fn params(&self, business: Value, expert: &[&ExpertParams]) -> Result<Value, String> {
        let mut out: Map<String, Value> = self.specs.iter().map(|(k, s)| (k.clone(), s["default"].clone())).collect();
        for (name, value) in business.as_object().ok_or("测量图业务参数不是对象")? {
            match self.specs.get(name) {
                Some(spec) if !is_expert(spec) => out.insert(name.clone(), value.clone()),
                _ => return Err(format!("测量图模板 {} 没有业务参数 {name}", self.id)),
            };
        }
        for layer in expert {
            for (name, value) in *layer {
                if self.specs.get(name).is_some_and(is_expert) {
                    out.insert(name.clone(), value.clone());
                }
            }
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expert(pairs: &[(&str, f64)]) -> ExpertParams {
        pairs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()
    }

    #[test]
    fn camera_override_beats_recipe_default_and_unset_keys_keep_template_default() {
        let recipe = expert(&[("contrastMin", 24.0), ("mergeGap", 4.0)]);
        let camera = expert(&[("contrastMin", 32.0)]);
        let p = corridor().params(json!({}), &[&recipe, &camera]).unwrap();
        assert_eq!((p["contrastMin"].as_f64(), p["mergeGap"].as_f64(), p["presentRatio"].as_f64()), (Some(32.0), Some(4.0), Some(0.5)));
    }

    #[test]
    fn invalid_expert_params_are_rejected() {
        for (name, value) in [("noSuchParam", json!(1.0)), ("contrastMin", json!(0.0)), ("presentRatio", json!(1.5)), ("mergeGap", json!("8"))] {
            let params: ExpertParams = [(name.to_string(), value)].into_iter().collect();
            assert!(validate_expert(&params, "配方").is_err(), "{name} 应被拒绝");
        }
    }
}
