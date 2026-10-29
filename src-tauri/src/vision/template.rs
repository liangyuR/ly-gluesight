//! 测量图模板（LF-4）：GraphDoc 加顶层图参数，随程序版本发布。每次运行由 GlueSight 合成全部有效参数，经 params_json 传给 lyFlow；
//! 顶层图参数只放绑定到算子参数的项：标 `advanced` 的是专家参数（LF-5），其余由 GlueSight 按业务参数换算，不对外开放。
//! GlueSight 判定用的门限不绑定算子，放在模板扩展位 `x.gluesight.judge`（同一套规格字段，都是专家参数）。
//! 两类专家参数都由配方默认、相机覆盖逐层叠加。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// 一张测量图模板：GraphDoc 原文、解析出的顶层图参数规格与判定门限规格。
pub struct Template {
    pub id: &'static str,
    pub text: &'static str,
    pub doc: Value,
    specs: Map<String, Value>,
    judge: Map<String, Value>,
}

/// 配方里的专家参数（模板参数名 → 值）。
pub type ExpertParams = BTreeMap<String, Value>;

fn load(id: &'static str, text: &'static str) -> Template {
    let doc: Value =
        serde_json::from_str(text).unwrap_or_else(|e| panic!("测量图模板 {id} 不是合法 JSON：{e}"));
    let specs = doc["params"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("测量图模板 {id} 没有顶层图参数"));
    let judge = doc["x"]["gluesight"]["judge"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert!(
        judge.values().all(is_expert),
        "测量图模板 {id} 的判定门限必须标 advanced"
    );
    Template {
        id,
        text,
        doc,
        specs,
        judge,
    }
}

fn templates() -> &'static [Template; 2] {
    static ALL: OnceLock<[Template; 2]> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            load(
                "corridor",
                include_str!("../../graphs/corridor.lyflow.json"),
            ),
            load("heading", include_str!("../../graphs/heading.lyflow.json")),
        ]
    })
}

/// 示教走廊测量图（响应图 + `glue.polyline` → 两份 `glue.lateral_runs`，可接 `glue.part_edges`）。
pub fn corridor() -> &'static Template {
    &templates()[0]
}

/// 逐帧定向测量图：响应图只算一次，扇形搜索与回退（扇区 = 示教方向 ± 容差）各一支，每支 3 个候选方向
/// 各有 `glue.track_path` → `glue.lateral_runs` 与 `glue.part_edges`。
pub fn heading() -> &'static Template {
    &templates()[1]
}

pub fn all() -> impl Iterator<Item = &'static Template> {
    templates().iter()
}

fn is_expert(spec: &Value) -> bool {
    spec["advanced"] == true
}

/// 各模板的专家参数规格（顶层图参数里的专家参数与判定门限）；同名参数在不同模板里绑定不同节点，
/// 但规格（范围、默认值、说明）必须一致。
pub fn expert_specs() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for t in all() {
        for (name, spec) in t.specs.iter().chain(&t.judge) {
            if !is_expert(spec) {
                continue;
            }
            let mut spec = spec.clone();
            spec.as_object_mut().map(|s| s.remove("binds"));
            if let Some(other) = out.get(name) {
                assert_eq!(
                    other, &spec,
                    "测量图模板的专家参数 {name} 在各模板里规格不一致"
                );
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
        let spec = specs
            .get(name)
            .ok_or_else(|| format!("{what}：没有名为 {name} 的专家参数"))?;
        let label = spec["label"].as_str().unwrap_or(name);
        let number = value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{what}：专家参数「{label}」需为有限数"))?;
        let (lo, hi) = (
            spec["min"].as_f64().unwrap_or(f64::NEG_INFINITY),
            spec["max"].as_f64().unwrap_or(f64::INFINITY),
        );
        if !(lo..=hi).contains(&number) {
            return Err(format!(
                "{what}：专家参数「{label}」{number} 需在 {lo}–{hi} 之间"
            ));
        }
    }
    Ok(())
}

impl Template {
    /// 这次运行的全部有效参数：模板默认值，叠上 GlueSight 换算的业务参数（`business`，必须是非专家参数），
    /// 再按顺序叠上各层专家参数（配方默认、相机覆盖；只取本模板有的专家参数）。
    pub fn params(&self, business: Value, expert: &[&ExpertParams]) -> Result<Value, String> {
        let mut out: Map<String, Value> = self
            .specs
            .iter()
            .map(|(k, s)| (k.clone(), s["default"].clone()))
            .collect();
        for (name, value) in business.as_object().ok_or("测量图业务参数不是对象")? {
            match self.specs.get(name) {
                Some(spec) if !is_expert(spec) => out.insert(name.clone(), value.clone()),
                _ => return Err(format!("测量图模板 {} 没有业务参数 {name}", self.id)),
            };
        }
        overlay(&mut out, &self.specs, expert);
        Ok(Value::Object(out))
    }

    /// 这次运行的判定门限：`x.gluesight.judge` 的默认值，按顺序叠上各层专家参数（只取本模板有的门限）。
    pub fn judge(&self, expert: &[&ExpertParams]) -> Value {
        let mut out: Map<String, Value> = self
            .judge
            .iter()
            .map(|(k, s)| (k.clone(), s["default"].clone()))
            .collect();
        overlay(&mut out, &self.judge, expert);
        Value::Object(out)
    }
}

fn overlay(out: &mut Map<String, Value>, specs: &Map<String, Value>, expert: &[&ExpertParams]) {
    for layer in expert {
        for (name, value) in *layer {
            if specs.get(name).is_some_and(is_expert) {
                out.insert(name.clone(), value.clone());
            }
        }
    }
}

/// 有效参数（`Template::params` 或 `Template::judge` 的结果）里的一个数。
pub fn number(params: &Value, name: &str) -> Result<f64, String> {
    params[name]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("测量图参数 {name} 不是有限数"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expert(pairs: &[(&str, f64)]) -> ExpertParams {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect()
    }

    #[test]
    fn camera_override_beats_recipe_default_for_graph_params_and_judge_thresholds() {
        let recipe = expert(&[
            ("segmentMin", 12.0),
            ("mergeGap", 4.0),
            ("presentRatio", 0.4),
            ("centerRatio", 0.5),
        ]);
        let camera = expert(&[("segmentMin", 16.0), ("presentRatio", 0.6)]);
        let p = corridor().params(json!({}), &[&recipe, &camera]).unwrap();
        assert_eq!(
            (
                p["segmentMin"].as_f64(),
                p["mergeGap"].as_f64(),
                p["peakMin"].as_f64()
            ),
            (Some(16.0), Some(4.0), Some(16.0))
        );
        let j = corridor().judge(&[&recipe, &camera]);
        assert_eq!(
            (
                j["presentRatio"].as_f64(),
                j["centerRatio"].as_f64(),
                j["contrastRatio"].as_f64()
            ),
            (Some(0.6), Some(0.5), Some(0.3))
        );
        assert!(
            p.get("presentRatio").is_none() && j.get("segmentMin").is_none(),
            "判定门限不进图参数，图参数不进判定门限"
        );
    }

    #[test]
    fn invalid_or_retired_expert_params_are_rejected() {
        // contrastMin、pathMinCoverage、window 是 1.1 测量图的专家参数名，不迁移
        for (name, value) in [
            ("contrastMin", json!(16.0)),
            ("pathMinCoverage", json!(0.3)),
            ("window", json!(30.0)),
            ("peakMin", json!(-1.0)),
            ("presentRatio", json!(1.5)),
            ("mergeGap", json!("8")),
        ] {
            let params: ExpertParams = [(name.to_string(), value)].into_iter().collect();
            assert!(validate_expert(&params, "配方").is_err(), "{name} 应被拒绝");
        }
        let judge: ExpertParams = [
            ("minCoverage".to_string(), json!(0.5)),
            ("trackSharpMin".to_string(), json!(0.6)),
        ]
        .into_iter()
        .collect();
        assert!(validate_expert(&judge, "配方").is_ok());
    }
}
