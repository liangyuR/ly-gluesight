use serde_json::{json, Value};

pub(crate) fn teaching_source(source: &str, current_round: &str) -> &'static str {
    match source
        .strip_prefix("capture:")
        .and_then(|s| s.rsplit_once('#'))
    {
        Some((round, _)) if round == current_round => "currentTeachingPiece",
        Some(_) => "carried",
        None => "unknown",
    }
}

pub(crate) fn compact(status: &Value) -> Value {
    let empty = Vec::new();
    let cameras = status["cameras"].as_array().unwrap_or(&empty);
    let mut total = 0;
    let mut saved = 0;
    let mut color_usable = 0;
    let mut gray = 0;
    let mut todo = Vec::new();
    for camera in cameras {
        for segment in camera["segments"].as_array().unwrap_or(&empty) {
            total += 1;
            let mut reasons = Vec::new();
            if segment["saved"] == true {
                saved += 1;
            } else {
                reasons.push("corridorMissing");
            }
            if segment["measure"] == "gray" {
                gray += 1;
            } else if segment["modelUsable"] == true {
                color_usable += 1;
            } else {
                reasons.push("colorModelUnusable");
            }
            if segment["teachingSource"] == "carried" && segment["carryAccepted"] != true {
                reasons.push("carriedFrameUnconfirmed");
            }
            if segment["teachingSource"] == "unknown" && segment["carryAccepted"] != true {
                reasons.push("frameSourceUnknown");
            }
            if !reasons.is_empty() {
                let index = &segment["segment"];
                let detail = status["segments"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .find(|s| &s["index"] == index);
                let coverage = status["coverage"]["segments"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .find(|s| &s["segment"] == index);
                let range = coverage
                    .and_then(|s| s["cameras"].as_array())
                    .and_then(|cs| cs.iter().find(|c| c["camera"] == camera["camera"]))
                    .and_then(|c| c["ranges"].as_array())
                    .and_then(|ranges| {
                        ranges.iter().max_by_key(|r| {
                            r["lastSeq"]
                                .as_u64()
                                .unwrap_or(0)
                                .saturating_sub(r["firstSeq"].as_u64().unwrap_or(0))
                        })
                    });
                let recommended = range.and_then(|r| {
                    Some(
                        r["firstSeq"].as_u64()?
                            + (r["lastSeq"].as_u64()? - r["firstSeq"].as_u64()?) / 2,
                    )
                });
                todo.push(json!({
                    "camera": camera["camera"], "segment": index, "reasons": reasons,
                    "frames": detail.map(|s| &s["frames"]), "seq": segment["seq"],
                    "recommendedSeq": recommended, "modelIssues": segment["modelIssues"],
                }));
            }
        }
    }
    let zero: Vec<Value> = status["coverage"]["zeroCoverageSegments"].as_array().unwrap_or(&empty).iter().map(|index| {
        let regions: Vec<&Value> = status["coverage"]["regions"].as_array().unwrap_or(&empty).iter()
            .filter(|r| r["kind"]["type"] == "skipped" && &r["kind"]["segment"] == index).collect();
        json!({"segment": index, "reason": if regions.is_empty() { "noInspectionCoverage" } else { "plannedSkip" }, "regions": regions})
    }).collect();
    json!({
        "project": status["project"], "projectName": status["projectName"],
        "recipe": status["recipe"], "revision": status["revision"], "snapshot": status["snapshot"],
        "engine": status["engine"], "next": status["next"], "teachingPiece": status["teachingPiece"],
        "blockers": status["blockers"],
        "summary": {
            "teachingGroups": total, "savedGroups": saved, "usableColorGroups": color_usable,
            "grayGroups": gray, "todoGroups": todo.len(),
            "expectedCameraFrames": status["coverage"]["expectedCameraFrames"],
            "availableCameraFrames": status["coverage"]["availableCameraFrames"],
            "boundariesTrusted": status["coverage"]["boundariesTrusted"],
        },
        "passes": {
            "count": status["passes"]["count"], "skip": status["passes"]["skip"],
            "inRecipe": status["passes"]["inRecipe"], "history": status["passes"]["history"],
            "suggestion": { "current": status["passes"]["suggestion"]["current"],
                "suggested": status["passes"]["suggestion"]["suggested"], "pending": status["passes"]["suggestion"]["pending"] },
        },
        "todo": todo, "zeroCoverageReasons": zero,
        "acceptance": "notEvaluated",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_carried_frame_stays_todo_until_accepted_and_skipped_segment_has_no_recommended_frame()
    {
        let mut status = json!({
            "cameras": [{"camera":"cam1","segments":[{"segment":1,"saved":true,"measure":"color","modelUsable":true,"teachingSource":"carried","carryAccepted":false}]}],
            "segments": [{"index":1,"frames":[1,4]}],
            "coverage": {"zeroCoverageSegments":[1],"segments":[{"segment":1,"cameras":[{"camera":"cam1","ranges":[]}]}],"regions":[{"kind":{"type":"skipped","segment":1,"pass":1,"short":true},"firstSeq":1,"lastSeq":4}]}
        });
        status["cameras"][0]["segments"][0]["teachingSource"] =
            json!(teaching_source("capture:old-piece#8", "current-piece"));
        let result = compact(&status);
        assert_eq!(result["summary"]["todoGroups"], 1);
        assert_eq!(
            result["todo"][0]["reasons"],
            json!(["carriedFrameUnconfirmed"])
        );
        assert!(result["todo"][0]["recommendedSeq"].is_null());
        assert_eq!(result["zeroCoverageReasons"][0]["reason"], "plannedSkip");
        status["cameras"][0]["segments"][0]["carryAccepted"] = json!(true);
        assert_eq!(compact(&status)["summary"]["todoGroups"], 0);
    }
}
