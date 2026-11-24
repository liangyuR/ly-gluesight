use serde_json::{json, Value};

use super::judge::fit::{choose_path, PathGate};
use super::template;
use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{MixEvidence, MIX_STATION_THRESHOLD};
use crate::frame::MixCapture;
use crate::recipe::MixCheck;

#[derive(Debug, Clone)]
pub(crate) struct MixPlan {
    check: MixCheck,
    scale: f64,
    pub params: Value,
    pub retry: Option<Value>,
    station_threshold: f64,
    min_coverage: f64,
}

impl MixPlan {
    pub fn build(
        check: &MixCheck,
        scale: f64,
        base: &Value,
        heading: bool,
    ) -> Result<Self, String> {
        let model = check
            .model
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_default();
        let mut p = json!({
            "model": model, "backgroundRects": serde_json::to_string(&check.background_rects).map_err(|e| e.to_string())?,
            "roi": base["roi"], "halfWidthPx": base["searchHalf"].as_f64().ok_or("缺少搜索半宽")?.ceil() as u64
        });
        if !heading {
            p["points"] = base["points"].clone();
        }
        let retry = if heading {
            let mut params = json!({ "model": model });
            for key in [
                "nozzle",
                "zone",
                "radial",
                "sector",
                "roi",
                "pathWidthMax",
                "searchHalf",
            ] {
                params[key] = base[key].clone();
            }
            Some(template::mix_retrack().params(params)?)
        } else {
            None
        };
        Ok(Self {
            check: check.clone(),
            scale,
            params: template::mix().params(p)?,
            retry,
            station_threshold: MIX_STATION_THRESHOLD,
            min_coverage: template::number(&template::mix_retrack().judge(), "minCoverage")?,
        })
    }

    pub fn frozen(&self) -> Value {
        let mut entry = json!({"params": self.params, "stationThreshold": self.station_threshold});
        if let Some(retry) = &self.retry {
            entry["retryParams"] = retry.clone();
            entry["minCoverage"] = json!(self.min_coverage);
        }
        entry
    }

    pub fn restore(&mut self, entry: &Value, templates: &Value) -> Result<(), String> {
        let params = &entry["params"];
        check_frozen(template::mix(), &templates["mix"], params, &self.params)?;
        self.params = params.clone();
        self.station_threshold = template::number(entry, "stationThreshold")?;
        if let Some(current) = &self.retry {
            let retry = &entry["retryParams"];
            check_frozen(
                template::mix_retrack(),
                &templates["mix_retrack"],
                retry,
                current,
            )?;
            self.retry = Some(retry.clone());
            self.min_coverage = template::number(entry, "minCoverage")?;
        }
        Ok(())
    }

    pub fn measure(
        &self,
        engine: &Engine,
        image: &FrameImage,
        capture: Option<&MixCapture>,
        line: Option<&[[f32; 2]]>,
        run: &str,
    ) -> MixEvidence {
        self.observe(engine, image, capture, line, run)
            .unwrap_or_else(|reason| MixEvidence::unavailable(&self.check, reason))
    }

    fn observe(
        &self,
        engine: &Engine,
        image: &FrameImage,
        capture: Option<&MixCapture>,
        line: Option<&[[f32; 2]]>,
        run_id: &str,
    ) -> Result<MixEvidence, String> {
        if !image.is_color() {
            return Err("混胶色差需要彩色原图".into());
        }
        let model = self.check.model.as_ref().ok_or("尚未示教混胶色度模型")?;
        let capture = capture.ok_or("该帧缺少可核对的相机序列号与白平衡回读")?;
        if self.check.camera_serial.as_deref() != Some(capture.serial.as_str()) {
            return Err("采集相机序列号与混胶模型不一致".into());
        }
        if (model["datasetKind"] == "synthetic") != capture.synthetic {
            return Err("混胶模型与帧的模拟/实拍来源不一致".into());
        }
        if serde_json::from_value(model["whiteBalance"].clone()).ok() != Some(capture.white_balance)
        {
            return Err("该帧的白平衡与混胶模型不一致".into());
        }
        let run = |graph: &str, params: &Value| -> Result<RunResult, String> {
            let out = engine.run(graph, run_id, "", image, params)?;
            if out.status() == "ok" {
                Ok(out)
            } else {
                Err(out.failure())
            }
        };
        let mut params = self.params.clone();
        if let Some(retry) = &self.retry {
            params["points"] = if let Some(line) = line.filter(|l| l.len() >= 2) {
                json!(serde_json::to_string(line).map_err(|e| e.to_string())?)
            } else {
                let out = run(template::mix_retrack().text, retry)?;
                let scan = out.record("scan").ok_or("缺少混胶得分扇扫观测")?;
                let tracks = (0..3)
                    .map(|i| {
                        out.record(&format!("track{i}"))
                            .cloned()
                            .ok_or("缺少混胶跟踪观测")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let choice = choose_path(
                    scan,
                    &tracks,
                    PathGate {
                        min_coverage: self.min_coverage,
                        sharp_min: None,
                    },
                )?;
                let k = choice
                    .chosen
                    .filter(|_| choice.ok)
                    .ok_or_else(|| format!("混胶得分重定位未找到可靠胶路：{}", choice.reason))?;
                let points = &out
                    .record(&format!("line{k}"))
                    .ok_or("混胶重定位缺少胶路")?["points"];
                if points.as_array().is_none_or(|p| p.len() < 2) {
                    return Err("重定位胶路点不足".into());
                }
                json!(points.to_string())
            };
        }
        let out = run(template::mix().text, &params)?;
        let observation = out.record("observation").ok_or("混胶区域量测未返回观测")?;
        MixEvidence::from_observation(
            observation.clone(),
            &self.check,
            self.scale,
            self.station_threshold,
        )
    }
}

fn check_frozen(
    template: &template::Template,
    frozen: &Value,
    params: &Value,
    current: &Value,
) -> Result<(), String> {
    let specs = template.doc["params"]
        .as_object()
        .expect("内置图缺少参数表");
    let compatible = ["schemaVersion", "nodes", "edges", "outputs"]
        .iter()
        .all(|&k| frozen[k] == template.doc[k])
        && frozen["params"]
            .as_object()
            .is_some_and(|p| p.len() == specs.len())
        && params.as_object().is_some_and(|p| p.len() == specs.len())
        && specs.iter().all(|(k, s)| {
            frozen["params"][k]["binds"] == s["binds"]
                && params.get(k).is_some()
                && (s["advanced"] == true || params[k] == current[k])
        });
    if compatible {
        Ok(())
    } else {
        Err(format!(
            "发布包的 {} 图结构、参数绑定或配方几何不兼容，请重新验证发布",
            template.id
        ))
    }
}
