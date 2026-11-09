//! 测量图模板（LF-4）：GraphDoc 加顶层图参数，随程序版本发布。每次运行由 GlueSight 合成全部有效参数，经 params_json 传给 lyFlow；
//! 顶层图参数只放绑定到算子参数的项：标 `advanced` 的是算子门限，取模板默认值（流程调试页可临时改了试跑），其余由 GlueSight
//! 按业务参数换算。GlueSight 判定用的门限不绑定算子，放在模板扩展位 `x.gluesight.judge`（同一套规格字段，都标 `advanced`）。
//! 配方不存这些门限。

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

impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Template({})", self.id)
    }
}

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

fn templates() -> &'static [Template; 5] {
    static ALL: OnceLock<[Template; 5]> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            load(
                "corridor",
                include_str!("../../graphs/corridor.lyflow.json"),
            ),
            load("heading", include_str!("../../graphs/heading.lyflow.json")),
            load(
                "corridor_color",
                include_str!("../../graphs/corridor_color.lyflow.json"),
            ),
            load(
                "heading_color",
                include_str!("../../graphs/heading_color.lyflow.json"),
            ),
            load(
                "color_teach",
                include_str!("../../graphs/color_teach.lyflow.json"),
            ),
        ]
    })
}

/// 示教走廊测量图（响应图 + `glue.polyline` → 两份 `glue.lateral_runs`，可接 `glue.part_edges`）。
pub fn corridor() -> &'static Template {
    &templates()[0]
}

/// 逐帧定向测量图：响应图之后扇形搜索（扇区 = 出胶点背离最近画面边的半圈），3 个候选方向
/// 各有 `glue.track_path` → `glue.lateral_runs` 与 `glue.part_edges`。
pub fn heading() -> &'static Template {
    &templates()[1]
}

/// 胶色测量的示教走廊图：`glue.color_map`（颜色模型）+ `glue.polyline` → 两份 `glue.color_runs`，可接 `glue.part_edges`。
pub fn corridor_color() -> &'static Template {
    &templates()[2]
}

/// 胶色测量的逐帧定向图：`glue.color_map` 之后 `glue.color_fan`，3 个候选方向各有 `glue.color_track` →
/// `glue.color_runs` 与 `glue.part_edges`。
pub fn heading_color() -> &'static Template {
    &templates()[3]
}

/// 示教取色图：示教走廊（`glue.polyline`）→ `glue.color_model`，输出颜色模型。
pub fn color_teach() -> &'static Template {
    &templates()[4]
}

fn is_expert(spec: &Value) -> bool {
    spec["advanced"] == true
}

impl Template {
    /// 这次运行的全部有效参数：模板默认值，叠上 GlueSight 换算的业务参数（`business`，不能是标 `advanced` 的门限）。
    pub fn params(&self, business: Value) -> Result<Value, String> {
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
        Ok(Value::Object(out))
    }

    /// 这次运行的判定门限：`x.gluesight.judge` 的默认值。
    pub fn judge(&self) -> Value {
        Value::Object(
            self.judge
                .iter()
                .map(|(k, s)| (k.clone(), s["default"].clone()))
                .collect(),
        )
    }
}

/// 有效参数（`Template::params` 或 `Template::judge` 的结果）里的一个数。
pub fn number(params: &Value, name: &str) -> Result<f64, String> {
    params[name]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("测量图参数 {name} 不是有限数"))
}
