//! PlanGraph：三端共用的「目标 + 任务 + 依赖」中间模型，以及 → WorkflowDefinition 的确定性转换。
//!
//! 现状用途：**会话的计划清单 → 可运行工作流定义**（群聊任务表沿用其自身的转换器，
//! 后续「会话 → 群聊」也复用本模型，见各通道的 source adapter）。
//!
//! 组装依据与 `commands::groupchat::build_groupchat_workflow` 完全一致（每条都对应运行期真实语义，
//! 错一条就变成"能跑但拿不到数据"）：
//! - 每个节点都要 outputMapping：引擎按"入边源节点是否在上下文里有产出"判定边是否连通，
//!   没有映射的节点会让整条下游链被跳过；
//! - 起始节点用 outputMapping 承载总目标（引擎会把起始节点映射字段平铺进上下文），
//!   于是任意节点都能以 `{{goal}}` 取到总目标；
//! - 依赖即任务 DAG：无前置 → 起始节点，有前置 → 各前置任务节点，无后继 → 结束节点；
//! - 前置产出靠 inputMapping + 提示词占位符注入（引擎不自动拼接上游产出）；
//! - 引用表达式 `{{<字段名>.<节点ID>.<阶段ID>}}`，首段必须是**被引用节点 outputMapping 暴露的字段名**。

use crate::workflow::{
    GateConfig, Stage, TriggerConfig, TriggerType, WorkflowDefinition, WorkflowEdge, WorkflowNode,
    WorkflowNodeType,
};

/// 计划中的单个任务
#[derive(Debug, Clone)]
pub struct PlanTask {
    /// 稳定标识（同一 PlanGraph 内唯一），用于引用与依赖定位
    pub key: String,
    /// 展示序号，用于提示词里的 `{{t<n>}}` 占位符
    pub no: i64,
    /// 任务标题（进节点 label 与提示词）
    pub title: String,
    /// 任务详情（进提示词；为空时退回标题）
    pub detail: String,
    /// 前置任务 key（必须在同一 PlanGraph 内）
    pub deps: Vec<String>,
    /// Agent 节点运行期参数（agent_type / api_provider / api_model 等；prompt_template 由本模块生成）
    pub params: serde_json::Value,
    /// 是否为**确认类任务**（需要用户拍板才能继续）。
    ///
    /// true 时该任务不生成 Agent 节点，而生成「人工交互」节点：运行到此处挂起等用户作答，
    /// 答复随 outputMapping 的 `result` 流向下游——否则这类任务在无人值守的工作流里只会空转
    /// （Agent 还会白白等一次 60s 确认超时）。
    pub confirm: bool,
    /// 确认项的**默认值**（仅 confirm=true 时有意义）：无人应答时交互节点按它继续。
    ///
    /// 自动运行场景下这是"不空转"的关键——超时后拿到的不是空串，而是会话里推断出的取值。
    pub default_value: Option<String>,
}

/// 目标 + 任务 + 依赖（三端共用的中间模型）
#[derive(Debug, Clone)]
pub struct PlanGraph {
    /// 生成的定义名（会话标题 / 房间标题）
    pub name: String,
    /// 定义描述（含来源与结论，供列表与追溯）
    pub description: String,
    /// 总目标（进起始节点 outputMapping，任意节点可 `{{goal}}` 取到）
    pub goal: String,
    /// 阶段名
    pub stage_name: String,
    pub tasks: Vec<PlanTask>,
}

/// 规范的引用表达式 `{{<字段名>.<节点ID>.<阶段ID>}}`
pub fn reference(field: &str, node_id: &str, stage_id: &str) -> String {
    format!("{{{{{}.{}.{}}}}}", field, node_id, stage_id)
}

/// 按字符（而非字节）截断，避免中文被截半。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

/// 节点在阶段内容区（480×500）内的自适应排布（与群聊转换器同款口径）：
/// ≤6 单列、更多两列（列距 200）；行数 ≤6 时行距 80，否则 60。
fn node_position(index: usize, total: usize) -> (i64, i64) {
    let columns: usize = if total <= 6 { 1 } else { 2 };
    let rows = total.div_ceil(columns);
    let pitch: i64 = if rows <= 6 { 80 } else { 60 };
    let col = (index % columns) as i64;
    let row = (index / columns) as i64;
    let x = if columns == 1 { 20 } else { 20 + col * 200 };
    (x, 20 + row * pitch)
}

fn new_edge(source: &str, target: &str) -> WorkflowEdge {
    WorkflowEdge {
        id: format!("edge_{}_{}", source, target),
        source: source.to_string(),
        target: target.to_string(),
        label: None,
        condition: None,
    }
}

/// 单个任务的提示词模板：总目标 + 子任务 + 职责边界 + 前置产出占位符。
/// 占位符 `{{t<n>}}` 必须与 inputMapping 的键一一对应，否则展开后取值落空。
///
/// 「职责边界」是流水线型任务（写提示词 → 选参数 → 生成图片/视频…）不空转的关键：
/// 上游准备类节点一旦"顺手"把最终产物也做了，下游节点就没活可干（只剩复述）。
fn build_task_prompt(task: &PlanTask, deps: &[&PlanTask]) -> String {
    let body = if task.detail.trim().is_empty() {
        task.title.as_str()
    } else {
        task.detail.as_str()
    };
    let mut s = format!(
        "总目标：{{{{goal}}}}\n\n\
         你的子任务（T{}）：{}\n\n\
         【职责边界】本工作流按任务清单分步执行，你只负责 T{} 这一步：交付它自己该交付的东西\
         （例如「写提示词」就只交付提示词文本，「确定调用的 API/参数」就只交付 API 与参数），\
         不要替后续任务把它们的工作做完，也不要越界产出最终产物（图片/视频/音频/文件等），\
         除非本任务就是产出最终产物的那一步。\n",
        task.no, body, task.no
    );
    if !deps.is_empty() {
        s.push_str("\n【前置任务产出】\n");
        for d in deps {
            s.push_str(&format!(
                "- T{} {}：{{{{t{}}}}}\n",
                d.no,
                truncate_chars(&d.title, 24),
                d.no
            ));
        }
    }
    s
}

/// 确认类任务 → 「人工交互」节点配置（`HumanInputConfig` 的 camelCase 形态）。
///
/// prompt 就是用户看到的提问（取任务说明，为空退回标题）；有默认值时一并带上：
/// 无人应答（超时）时交互节点按默认值继续，自动运行的工作流因此不会拿到空串而中断语义。
/// 默认值留空则不写 defaultValue —— 行为与手建的交互节点一致（超时取空串）。
fn build_interact_params(task: &PlanTask) -> serde_json::Value {
    let body = if task.detail.trim().is_empty() {
        task.title.as_str()
    } else {
        task.detail.as_str()
    };
    let default_value = task
        .default_value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let mut m = serde_json::Map::new();
    m.insert("prompt".into(), serde_json::json!(body));
    m.insert("inputType".into(), serde_json::json!("text"));
    m.insert(
        "placeholder".into(),
        serde_json::json!("请填写确认结果（超时按默认值继续）"),
    );
    if let Some(d) = default_value {
        m.insert("defaultValue".into(), serde_json::json!(d));
    }
    serde_json::Value::Object(m)
}

/// 计划 → 工作流定义（确定性；依赖成环/引用缺失直接报错）。
pub fn build_definition(plan: &PlanGraph) -> Result<WorkflowDefinition, String> {
    if plan.tasks.is_empty() {
        return Err("计划里没有任务，无法生成工作流".to_string());
    }

    // 依赖成环检测（Kahn）：入度归零反复取出，取不完即有环
    {
        let mut indeg: std::collections::HashMap<&str, usize> = plan
            .tasks
            .iter()
            .map(|t| (t.key.as_str(), 0usize))
            .collect();
        if indeg.len() != plan.tasks.len() {
            return Err("计划里存在重复的任务标识".to_string());
        }
        for t in &plan.tasks {
            for d in &t.deps {
                if !indeg.contains_key(d.as_str()) {
                    return Err(format!("任务 T{} 的前置 '{}' 不在计划内", t.no, d));
                }
            }
            *indeg.get_mut(t.key.as_str()).unwrap() += t.deps.len();
        }
        let mut ready: Vec<&str> = indeg
            .iter()
            .filter(|(_, v)| **v == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut done = 0usize;
        while let Some(k) = ready.pop() {
            done += 1;
            for t in &plan.tasks {
                if t.deps.iter().any(|d| d == k) {
                    let e = indeg.get_mut(t.key.as_str()).unwrap();
                    *e -= 1;
                    if *e == 0 {
                        ready.push(t.key.as_str());
                    }
                }
            }
        }
        if done != plan.tasks.len() {
            return Err("任务依赖存在环，无法生成工作流".to_string());
        }
    }

    let stage_id = uuid::Uuid::new_v4().to_string();
    let start_id = "node_start".to_string();
    let end_id = "node_end".to_string();
    let node_id_of = |key: &str| format!("node_{}", key);
    let total = plan.tasks.len() + 2;

    let mut nodes: Vec<WorkflowNode> = Vec::new();
    let mut edges: Vec<WorkflowEdge> = Vec::new();

    // 起始节点：outputMapping 承载总目标
    let (sx, sy) = node_position(0, total);
    nodes.push(WorkflowNode {
        id: start_id.clone(),
        node_type: WorkflowNodeType::Start,
        label: "开始".to_string(),
        plugin_id: None,
        command_id: None,
        params: None,
        delay_ms: None,
        timeout_ms: None,
        input_schema: None,
        output_schema: None,
        input_mapping: None,
        output_mapping: Some(serde_json::json!({ "goal": plan.goal })),
        position: Some(serde_json::json!({ "x": sx, "y": sy })),
    });

    // 任务节点
    for (i, t) in plan.tasks.iter().enumerate() {
        let nid = node_id_of(&t.key);
        let deps: Vec<&PlanTask> = t
            .deps
            .iter()
            .filter_map(|d| plan.tasks.iter().find(|x| &x.key == d))
            .collect();

        // 确认类任务 → 「人工交互」节点（配置与编辑器里的同名节点同构：prompt / inputType）；
        // 其余任务 → Agent 节点（提示词模板由本模块生成）。
        let (node_type, params) = if t.confirm {
            (WorkflowNodeType::Interact, build_interact_params(t))
        } else {
            let mut params = t.params.clone();
            if !params.is_object() {
                params = serde_json::json!({});
            }
            params["prompt_template"] = serde_json::json!(build_task_prompt(t, &deps));
            (WorkflowNodeType::Agent, params)
        };

        let mut mapping = serde_json::Map::new();
        mapping.insert(
            "goal".to_string(),
            serde_json::json!(reference("goal", &start_id, &stage_id)),
        );
        for d in &deps {
            mapping.insert(
                format!("t{}", d.no),
                serde_json::json!(reference("result", &node_id_of(&d.key), &stage_id)),
            );
        }

        let (x, y) = node_position(i + 1, total);
        nodes.push(WorkflowNode {
            id: nid.clone(),
            node_type,
            label: format!("T{} {}", t.no, truncate_chars(&t.title, 24)),
            plugin_id: None,
            command_id: None,
            params: Some(params),
            delay_ms: None,
            timeout_ms: None,
            input_schema: None,
            output_schema: None,
            input_mapping: Some(serde_json::Value::Object(mapping)),
            // 两类节点都暴露 result：既作下游 `{{…result}}` 数据源，也让本节点出边被判定为连通
            // （agent 的 output 是裸文本、interact 是 `{content: 用户答复}`，`{{content}}` 两边都取得到）
            output_mapping: Some(serde_json::json!({ "result": "{{content}}" })),
            position: Some(serde_json::json!({ "x": x, "y": y })),
        });

        if deps.is_empty() {
            edges.push(new_edge(&start_id, &nid));
        } else {
            for d in &deps {
                edges.push(new_edge(&node_id_of(&d.key), &nid));
            }
        }
    }

    // 结束节点 + 无后继任务的收口边
    let (ex, ey) = node_position(plan.tasks.len() + 1, total);
    nodes.push(WorkflowNode {
        id: end_id.clone(),
        node_type: WorkflowNodeType::End,
        label: "结束".to_string(),
        plugin_id: None,
        command_id: None,
        params: None,
        delay_ms: None,
        timeout_ms: None,
        input_schema: None,
        output_schema: None,
        input_mapping: None,
        output_mapping: None,
        position: Some(serde_json::json!({ "x": ex, "y": ey })),
    });
    for t in &plan.tasks {
        let has_succ = plan
            .tasks
            .iter()
            .any(|x| x.deps.iter().any(|d| d == &t.key));
        if !has_succ {
            edges.push(new_edge(&node_id_of(&t.key), &end_id));
        }
    }

    let now = crate::utils::now();
    Ok(WorkflowDefinition {
        id: uuid::Uuid::new_v4().to_string(),
        name: plan.name.clone(),
        version: "1.0.0".to_string(),
        description: plan.description.clone(),
        trigger: TriggerConfig {
            trigger_type: TriggerType::Manual,
            cron: None,
            event_name: None,
        },
        stages: vec![Stage {
            id: stage_id,
            name: plan.stage_name.clone(),
            order: 0,
            nodes,
            edges,
            stage_edges: Vec::new(),
            gate: GateConfig::default(),
            collapsed: false,
            offset_x: 0.0,
            offset_y: 0.0,
        }],
        icon: None,
        input_schema: None,
        output_schema: None,
        created_at: now,
        updated_at: now,
        enabled: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(key: &str, no: i64, deps: &[&str]) -> PlanTask {
        PlanTask {
            key: key.to_string(),
            no,
            title: format!("任务{}", no),
            detail: format!("做第{}件事", no),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            params: serde_json::json!({
                "agent_type": "api", "api_provider": "p1", "api_model": "m1"
            }),
            confirm: false,
            default_value: None,
        }
    }

    /// 确认类任务必须生成「人工交互」节点（配置带 prompt），而不是 Agent 节点：
    /// 否则这类节点在无人值守的工作流里只会空转。
    #[test]
    fn confirm_task_becomes_interact_node() {
        let mut t1 = task("t1", 1, &[]);
        t1.confirm = true;
        // 默认值：无人应答时交互节点按它继续（自动运行不空转）
        t1.default_value = Some("采用方案 B".to_string());
        let def = build_definition(&plan(vec![t1, task("t2", 2, &["t1"])])).unwrap();
        let stage = &def.stages[0];

        let n1 = stage.nodes.iter().find(|n| n.id == "node_t1").unwrap();
        assert_eq!(n1.node_type, WorkflowNodeType::Interact);
        let p = n1.params.as_ref().unwrap();
        assert_eq!(p["prompt"], "做第1件事");
        assert_eq!(p["defaultValue"], "采用方案 B");
        assert!(
            p.get("prompt_template").is_none(),
            "交互节点不该有提示词模板"
        );
        // 出边要能判定连通、下游要能取到答复，靠的仍是 result → {{content}}
        assert_eq!(n1.output_mapping.as_ref().unwrap()["result"], "{{content}}");

        let n2 = stage.nodes.iter().find(|n| n.id == "node_t2").unwrap();
        assert_eq!(n2.node_type, WorkflowNodeType::Agent);
        assert_eq!(
            n2.input_mapping.as_ref().unwrap()["t1"],
            format!("{{{{result.node_t1.{}}}}}", stage.id)
        );
    }

    /// 默认值留空（或只有空白）时不写 defaultValue：与手建交互节点的超时行为一致，
    /// 避免把空串当成"已确认的取值"写进节点配置。
    #[test]
    fn blank_default_value_is_not_written() {
        let mut t1 = task("t1", 1, &[]);
        t1.confirm = true;
        t1.default_value = Some("   ".to_string());
        let def = build_definition(&plan(vec![t1])).unwrap();
        let n1 = def.stages[0]
            .nodes
            .iter()
            .find(|n| n.id == "node_t1")
            .unwrap();
        assert!(n1.params.as_ref().unwrap().get("defaultValue").is_none());
    }

    fn plan(tasks: Vec<PlanTask>) -> PlanGraph {
        PlanGraph {
            name: "测试工作流".to_string(),
            description: "由会话转换".to_string(),
            goal: "把测试跑通".to_string(),
            stage_name: "会话任务".to_string(),
            tasks,
        }
    }

    #[test]
    fn chain_plan_builds_connected_dag() {
        let def = build_definition(&plan(vec![
            task("t1", 1, &[]),
            task("t2", 2, &["t1"]),
            task("t3", 3, &["t2"]),
        ]))
        .unwrap();
        let stage = &def.stages[0];
        assert_eq!(stage.nodes.len(), 5); // start + 3 任务 + end
        assert_eq!(stage.edges.len(), 4); // start→t1, t1→t2, t2→t3, t3→end

        let start = stage.nodes.iter().find(|n| n.id == "node_start").unwrap();
        assert_eq!(start.output_mapping.as_ref().unwrap()["goal"], "把测试跑通");

        for n in stage
            .nodes
            .iter()
            .filter(|n| n.node_type == WorkflowNodeType::Agent)
        {
            assert_eq!(n.output_mapping.as_ref().unwrap()["result"], "{{content}}");
            let tpl = n.params.as_ref().unwrap()["prompt_template"]
                .as_str()
                .unwrap();
            assert!(tpl.contains("{{goal}}"), "{}", tpl);
            // 职责边界：上游准备类节点不得越界产出最终产物（否则下游节点空转）
            assert!(tpl.contains("【职责边界】"), "{}", tpl);
            assert!(tpl.contains("不要越界产出最终产物"), "{}", tpl);
        }

        let t2 = stage.nodes.iter().find(|n| n.id == "node_t2").unwrap();
        let m = t2.input_mapping.as_ref().unwrap();
        assert_eq!(m["goal"], format!("{{{{goal.node_start.{}}}}}", stage.id));
        assert_eq!(m["t1"], format!("{{{{result.node_t1.{}}}}}", stage.id));
        assert!(t2.params.as_ref().unwrap()["prompt_template"]
            .as_str()
            .unwrap()
            .contains("{{t1}}"));
    }

    #[test]
    fn all_references_are_three_segment_and_resolvable() {
        let def =
            build_definition(&plan(vec![task("t1", 1, &[]), task("t2", 2, &["t1"])])).unwrap();
        let stage = &def.stages[0];
        let ids: Vec<&str> = stage.nodes.iter().map(|n| n.id.as_str()).collect();
        for n in &stage.nodes {
            if let Some(map) = n.input_mapping.as_ref().and_then(|v| v.as_object()) {
                for (_, v) in map {
                    let s = v.as_str().unwrap();
                    let inner = s.trim_start_matches("{{").trim_end_matches("}}");
                    let parts: Vec<&str> = inner.split('.').collect();
                    assert_eq!(parts.len(), 3, "引用必须是三段式: {}", s);
                    assert!(ids.contains(&parts[1]), "引用的节点不存在: {}", s);
                    assert_eq!(parts[2], stage.id);
                }
            }
        }
    }

    #[test]
    fn cycle_is_rejected() {
        let err = build_definition(&plan(vec![task("t1", 1, &["t2"]), task("t2", 2, &["t1"])]))
            .unwrap_err();
        assert!(err.contains("环"), "{}", err);
    }

    #[test]
    fn dangling_dependency_is_rejected() {
        let err = build_definition(&plan(vec![task("t1", 1, &["nope"])])).unwrap_err();
        assert!(err.contains("不在计划内"), "{}", err);
    }

    #[test]
    fn empty_plan_is_rejected() {
        assert!(build_definition(&plan(vec![])).is_err());
    }
}
