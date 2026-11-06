//! 界面层的单元测试。

use crate::prelude::*;

fn temp_root(tag: &str) -> Root {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    Root::new(std::env::temp_dir().join(format!("nailong-ui-{tag}-{}-{nanos}", std::process::id())))
}

#[test]
fn windows_browser_arg_keeps_full_oauth_query() {
    let url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x&scope=XboxLive.signin%20offline_access&state=abc";
    let args = browser_launch_args(url);
    assert_eq!(args[1], url);
    assert!(args[1].contains("&scope=XboxLive.signin"));
}

#[test]
fn login_errors_are_actionable_not_raw_aadsts() {
    let scope =
        friendly_login_error("微软登录失败：invalid_request（AADSTS900144: scope is missing）");
    assert!(scope.contains("更新"));
    assert!(!scope.contains("AADSTS"));
    assert_eq!(
        friendly_login_error("微软登录失败：invalid_client"),
        "启动器的微软登录配置暂时不可用，请稍后重试"
    );
    assert_eq!(friendly_login_error("已取消登录"), "已取消登录");
}

#[test]
fn detects_loader_base_dependencies() {
    let root = temp_root("deps");
    std::fs::create_dir_all(root.versions()).unwrap();
    let base = root.versions().join("1.21.1");
    let loader = root.versions().join("neoforge-21.1.209");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::create_dir_all(&loader).unwrap();
    std::fs::write(base.join("1.21.1.json"), r#"{"id":"1.21.1"}"#).unwrap();
    std::fs::write(
        loader.join("neoforge-21.1.209.json"),
        r#"{"id":"neoforge-21.1.209","inheritsFrom":"1.21.1"}"#,
    )
    .unwrap();
    let deps = profile_dependents(&root);
    assert_eq!(deps.get("1.21.1"), Some(&1));
    let _ = std::fs::remove_dir_all(root.dir());
}

#[test]
fn removal_is_direct_child_only() {
    let root = temp_root("remove");
    std::fs::create_dir_all(root.versions().join("old")).unwrap();
    std::fs::write(root.versions().join("old").join("x"), b"x").unwrap();
    root.remove_version("old").unwrap();
    assert!(!root.versions().join("old").exists());
    assert!(root.remove_version("../outside").is_err());
    assert!(root.remove_version(".").is_err());
    let _ = std::fs::remove_dir_all(root.dir());
}
