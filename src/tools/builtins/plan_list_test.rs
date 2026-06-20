use super::{
    PlanListEditArgs, PlanListEditItem, PlanListItem, PlanListUpdateArgs, PlanState, PlanStates,
    PlanStatus, apply_edit, computed_update_cost, current_time_ms, find_item_mut,
    mark_finalized_items, render_plan_message, validate_edit, validate_initial_items,
    validate_update,
};
/// 测试完成状态不需要模型传耗时，适用于由工具层计算 cost。
#[test]
fn validate_update_allows_done_without_cost() {
    let args = PlanListUpdateArgs {
        key: "build".to_string(),
        status: PlanStatus::Done,
        desc: None,
    };

    assert!(validate_update(&args).is_ok());
}

/// 测试失败状态不需要模型传耗时，适用于避免工具错误覆盖真实失败状态。
#[test]
fn validate_update_allows_failed_without_cost() {
    let args = PlanListUpdateArgs {
        key: "build".to_string(),
        status: PlanStatus::Failed,
        desc: None,
    };

    assert!(validate_update(&args).is_ok());
}

/// 测试终态耗时计算，适用于由工具层生成稳定 cost。
#[test]
fn computed_update_cost_formats_terminal_status() {
    assert_eq!(
        computed_update_cost(PlanStatus::Done, 12_000),
        Some("12s".to_string())
    );
    assert_eq!(
        computed_update_cost(PlanStatus::Failed, 185_000),
        Some("3m05s".to_string())
    );
}

/// 测试非终态不展示耗时，适用于只在完成或失败时显示 cost。
#[test]
fn computed_update_cost_omits_non_terminal_status() {
    assert_eq!(computed_update_cost(PlanStatus::Ing, 3_000), None);
    assert_eq!(computed_update_cost(PlanStatus::Wait, 3_000), None);
}

/// 测试进行中状态不需要耗时，适用于允许长任务持续刷新。
#[test]
fn validate_update_allows_ing_without_cost() {
    let args = PlanListUpdateArgs {
        key: "build".to_string(),
        status: PlanStatus::Ing,
        desc: Some("running".to_string()),
    };

    assert!(validate_update(&args).is_ok());
}

/// 测试计划消息渲染，适用于确认用户侧看到序号、标题、状态和耗时。
#[test]
fn render_plan_message_includes_status_cost_and_desc() {
    let rendered = render_plan_message(&[PlanListItem {
        title: "构建".to_string(),
        key: "build".to_string(),
        status: PlanStatus::Done,
        finalized: false,
        cost: Some("12s".to_string()),
        desc: Some("ok".to_string()),
        children: Vec::new(),
    }]);

    assert_eq!(rendered, "1. 构建 ✅ 12s\n  ok");
}

/// 测试嵌套计划渲染，适用于确认多级列表可读。
#[test]
fn render_plan_message_supports_nested_items() {
    let rendered = render_plan_message(&[PlanListItem {
        title: "构建".to_string(),
        key: "build".to_string(),
        status: PlanStatus::Ing,
        finalized: false,
        cost: None,
        desc: Some("处理中".to_string()),
        children: vec![PlanListItem {
            title: "编译".to_string(),
            key: "compile".to_string(),
            status: PlanStatus::Wait,
            finalized: false,
            cost: None,
            desc: None,
            children: Vec::new(),
        }],
    }]);

    assert_eq!(rendered, "1. 构建 ☕\n  1.1. 编译 🕒\n  处理中");
}

/// 测试点号路径查找，适用于更新任意层级计划项。
#[test]
fn find_item_mut_supports_dot_path() {
    let mut items = vec![PlanListItem {
        title: "构建".to_string(),
        key: "build".to_string(),
        status: PlanStatus::Ing,
        finalized: false,
        cost: None,
        desc: None,
        children: vec![PlanListItem {
            title: "编译".to_string(),
            key: "compile".to_string(),
            status: PlanStatus::Wait,
            finalized: false,
            cost: None,
            desc: None,
            children: Vec::new(),
        }],
    }];

    let item = find_item_mut(&mut items, "build.compile").expect("嵌套项必须存在");
    item.status = PlanStatus::Done;
    item.cost = Some("2s".to_string());

    assert_eq!(render_plan_message(&items), "1. 构建 ☕\n  1.1. 编译 ✅ 2s");
}

/// 测试初始计划 key 禁止点号，适用于保留点号路径语义。
#[test]
fn validate_initial_items_rejects_dot_in_key() {
    let items = vec![PlanListItem {
        title: "构建".to_string(),
        key: "build.compile".to_string(),
        status: PlanStatus::Wait,
        finalized: false,
        cost: None,
        desc: None,
        children: Vec::new(),
    }];

    assert!(validate_initial_items(&items).is_err());
}

/// 测试初始终态项会被标记，适用于防止 done/failed 重复更新。
#[test]
fn mark_finalized_items_marks_terminal_items() {
    let mut items = vec![PlanListItem {
        title: "构建".to_string(),
        key: "build".to_string(),
        status: PlanStatus::Done,
        finalized: false,
        cost: Some("1s".to_string()),
        desc: Some("ok".to_string()),
        children: Vec::new(),
    }];

    mark_finalized_items(&mut items);

    assert!(items[0].finalized);
}

/// 测试计划编辑新增项，适用于动态插入新的根级计划。
#[test]
fn apply_edit_inserts_new_root_item() {
    let mut items = vec![PlanListItem {
        title: "原计划".to_string(),
        key: "old".to_string(),
        status: PlanStatus::Wait,
        finalized: false,
        cost: None,
        desc: None,
        children: Vec::new(),
    }];

    apply_edit(
        &mut items,
        PlanListEditItem {
            new: true,
            index: Some(1),
            key: None,
            title: "新计划".to_string(),
        },
    )
    .expect("新增计划必须成功");

    assert_eq!(items[0].title, "新计划");
    assert_eq!(items[1].title, "原计划");
    assert_eq!(items[0].status, PlanStatus::Wait);
}

/// 测试计划编辑修改标题，适用于更新任意层级已有计划。
#[test]
fn apply_edit_renames_existing_nested_item() {
    let mut items = vec![PlanListItem {
        title: "构建".to_string(),
        key: "build".to_string(),
        status: PlanStatus::Wait,
        finalized: false,
        cost: None,
        desc: None,
        children: vec![PlanListItem {
            title: "旧编译".to_string(),
            key: "compile".to_string(),
            status: PlanStatus::Wait,
            finalized: false,
            cost: None,
            desc: None,
            children: Vec::new(),
        }],
    }];

    apply_edit(
        &mut items,
        PlanListEditItem {
            new: false,
            index: None,
            key: Some("build.compile".to_string()),
            title: "新编译".to_string(),
        },
    )
    .expect("修改计划必须成功");

    assert_eq!(items[0].children[0].title, "新编译");
}

/// 测试计划编辑参数校验，适用于防止模型漏传必要定位字段。
#[test]
fn validate_edit_requires_index_or_key_by_mode() {
    let missing_index = PlanListEditArgs {
        list: vec![PlanListEditItem {
            new: true,
            index: None,
            key: None,
            title: "新增".to_string(),
        }],
    };
    let missing_key = PlanListEditArgs {
        list: vec![PlanListEditItem {
            new: false,
            index: None,
            key: None,
            title: "修改".to_string(),
        }],
    };

    assert!(validate_edit(&missing_index).is_err());
    assert!(validate_edit(&missing_key).is_err());
}

/// 测试 provider 失败补偿，适用于将嵌套执行中计划改成失败态。
#[test]
fn fail_active_item_marks_nested_ing_item_failed() {
    let mut states = PlanStates::default();
    states.items.insert(
        "session".to_string(),
        PlanState {
            channel_name: "feishu".to_string(),
            chat_id: "chat".to_string(),
            message_id: "message".to_string(),
            reply_hash: Some("planhash".to_string()),
            last_update_at_ms: current_time_ms().saturating_sub(12_000),
            items: vec![PlanListItem {
                title: "根计划".to_string(),
                key: "root".to_string(),
                status: PlanStatus::Wait,
                finalized: false,
                cost: None,
                desc: None,
                children: vec![PlanListItem {
                    title: "子任务".to_string(),
                    key: "child".to_string(),
                    status: PlanStatus::Ing,
                    finalized: false,
                    cost: None,
                    desc: Some("处理中".to_string()),
                    children: Vec::new(),
                }],
            }],
        },
    );

    let update =
        futures_test_block_on(states.fail_active_item("session", "provider 502，上游请求失败"))
            .expect("失败补偿不能报错")
            .expect("必须找到执行中计划");

    assert_eq!(
        update.text,
        "[planhash]\n1. 根计划 🕒\n  1.1. 子任务 💥 12s\n    provider 502，上游请求失败"
    );
    assert!(
        futures_test_block_on(states.fail_active_item("session", "again"))
            .expect("失败补偿不能报错")
            .is_none()
    );
}

/// 测试计划状态持久化恢复，适用于 daemon 重启后继续 patch 原卡片。
#[test]
fn plan_states_restore_from_store_root() {
    let root = std::env::temp_dir().join(format!(
        "llm-loop-plan-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut states = PlanStates::default().with_store_root(root.clone());
    let state = PlanState {
        channel_name: "feishu".to_string(),
        chat_id: "chat".to_string(),
        message_id: "message".to_string(),
        reply_hash: Some("restorehash".to_string()),
        last_update_at_ms: current_time_ms(),
        items: vec![PlanListItem {
            title: "恢复计划".to_string(),
            key: "restore".to_string(),
            status: PlanStatus::Ing,
            finalized: false,
            cost: None,
            desc: None,
            children: Vec::new(),
        }],
    };

    futures_test_block_on(states.save_state("session", state)).expect("计划保存必须成功");
    let mut restored = PlanStates::default().with_store_root(root.clone());
    assert!(
        futures_test_block_on(restored.restore_state_if_needed("session"))
            .expect("计划恢复不能报错")
    );
    let update = futures_test_block_on(restored.fail_active_item("session", "provider 502"))
        .expect("失败补偿不能报错")
        .expect("恢复后必须能更新计划");

    assert_eq!(update.message_id, "message");
    assert!(update.text.contains("provider 502"));
    let _ = std::fs::remove_dir_all(root);
}

/// 同步等待异步测试逻辑，适用于无需 tokio 宏的轻量单测。
fn futures_test_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试 runtime 必须创建成功")
        .block_on(future)
}
