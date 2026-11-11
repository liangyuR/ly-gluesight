use std::collections::HashMap;

use serde_json::Value;

use super::super::Workspace;

pub(crate) fn guard_arguments(
    arguments: &mut Value,
    project: &str,
    workspaces: &HashMap<String, Workspace>,
) -> Result<Option<u64>, String> {
    let map = arguments.as_object_mut().ok_or("参数要是对象")?;
    if let Some(expected) = map.remove("expectedProject") {
        let expected = expected.as_str().ok_or("expectedProject 必须是项目编号")?;
        if expected != project {
            return Err(format!(
                "当前项目是 {project}，与预期项目 {expected} 不一致；请重新读取 teach_status"
            ));
        }
    }
    let instance = if let Some(expected) = map.remove("expectedInstanceId") {
        let expected = expected
            .as_u64()
            .ok_or("expectedInstanceId 必须是非负整数")?;
        let recipe = map
            .get("recipe")
            .and_then(Value::as_str)
            .ok_or("身份校验需要 recipe")?;
        let current = workspaces
            .get(recipe)
            .ok_or("候选不存在，请重新读取 teach_status")?;
        if current.instance_id != expected {
            return Err("候选已重建，请重新读取 teach_status".into());
        }
        Some(expected)
    } else {
        None
    };
    Ok(instance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::SegmentKind::Straight;
    use crate::workspace::burst_tests::burst_workspace;
    use serde_json::json;

    #[test]
    fn request_for_another_project_or_recreated_candidate_is_rejected_before_dispatch() {
        let mut workspace = burst_workspace(&[Straight]);
        workspace.instance_id = 12;
        let recipe = workspace.doc.id.clone();
        let workspaces = HashMap::from([(recipe.clone(), workspace)]);
        for (project, instance) in [("other-project", 12), ("current-project", 11)] {
            let mut arguments = json!({"recipe": recipe, "expectedProject": project, "expectedInstanceId": instance});
            assert!(guard_arguments(&mut arguments, "current-project", &workspaces).is_err());
        }
        let mut arguments = json!({"recipe": recipe, "expectedProject": "current-project", "expectedInstanceId": 12, "expectedRevision": 4});
        assert_eq!(
            guard_arguments(&mut arguments, "current-project", &workspaces).unwrap(),
            Some(12)
        );
        assert_eq!(arguments, json!({"recipe": recipe, "expectedRevision": 4}));
    }
}
