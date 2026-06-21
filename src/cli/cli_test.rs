use super::CliCommand;

/// 无参数时解析为 daemon 启动。
#[test]
fn parse_empty_args_as_daemon() {
    let command = CliCommand::parse(Vec::<String>::new()).expect("应能解析默认命令");

    assert_eq!(command, CliCommand::Daemon);
}

/// login 子命令解析为 OAuth 登录。
#[test]
fn parse_login_command() {
    let command = CliCommand::parse(vec!["login".to_string()]).expect("应能解析 login");

    assert_eq!(command, CliCommand::Login);
}

/// doctor 子命令解析为本地体检。
#[test]
fn parse_doctor_command() {
    let command = CliCommand::parse(vec!["doctor".to_string()]).expect("应能解析 doctor");

    assert_eq!(command, CliCommand::Doctor);
}

/// resources 子命令解析为运行态资源查询。
#[test]
fn parse_resources_command() {
    let command = CliCommand::parse(vec!["resources".to_string()]).expect("应能解析 resources");

    assert_eq!(command, CliCommand::Resources);
}

/// 未知子命令返回 CLI 错误。
#[test]
fn parse_unknown_command_as_error() {
    let error = CliCommand::parse(vec!["wat".to_string()]).expect_err("未知命令应报错");

    assert!(error.to_string().contains("unknown command"));
}
