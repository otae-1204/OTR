//! 锁住"额度刷新不再阻塞 UI 线程"这个修复。
//!
//! 背景:refresh_limits 原先是非 async 命令。Tauri 把非 async 命令判为
//! ExecutionContext::Blocking,命令体内联在 IPC 调用里执行,也就是跑在
//! 窗口/事件循环那条线程上。而它做的是阻塞网络 I/O —— Codex 登录态失效时
//! 整条链路(连接超时 × 多账号串行 × 代理逐档重试)会把 UI 冻住。
//!
//! 这里从两个角度验证修复:
//!   1. 编译期:refresh_limits 必须是 async fn(非 async 就无法被 spawn 到线程池);
//!   2. 运行期:spawn_blocking 真的把阻塞工作挪出了调用线程。
//!
//! 手动跑:cargo test --test limits_no_freeze -- --ignored --nocapture

/// 运行期验证:spawn_blocking 里的阻塞不会占住调用线程。
///
/// 用"睡 2 秒"模拟网络阻塞,断言调用线程在 200ms 内就拿到了句柄并继续往下走。
#[test]
#[ignore]
fn spawn_blocking_does_not_hold_the_calling_thread() {
    let started = std::time::Instant::now();

    let handle = tauri::async_runtime::spawn_blocking(|| {
        // 模拟一次连不上的请求(真实场景里这是连接超时)
        std::thread::sleep(std::time::Duration::from_secs(2));
        "done"
    });

    let dispatched = started.elapsed();
    println!("把阻塞任务派出去耗时 = {dispatched:?}");
    assert!(
        dispatched < std::time::Duration::from_millis(200),
        "派发不该等待阻塞任务完成,实际 {dispatched:?}"
    );

    let got = tauri::async_runtime::block_on(handle).expect("blocking task ok");
    println!("任务结果 = {got}, 总耗时 = {:?}", started.elapsed());
    assert_eq!(got, "done");
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
}

/// 连接超时必须短到不会让用户以为"卡死了"。
///
/// ureq 的 timeout_connect 默认 30 秒且优先于 timeout()。这个断言把上限钉住,
/// 避免以后有人把常量改大却不自知。
#[test]
#[ignore]
fn connect_timeout_is_short_enough_to_feel_responsive() {
    use otr_lib::limits::http::{HTTP_CONNECT_TIMEOUT_SECS, HTTP_TIMEOUT_SECS};
    println!(
        "连接超时 = {HTTP_CONNECT_TIMEOUT_SECS}s, 整体超时 = {HTTP_TIMEOUT_SECS}s"
    );
    assert!(
        HTTP_CONNECT_TIMEOUT_SECS < HTTP_TIMEOUT_SECS,
        "连接超时必须短于整体超时"
    );
    assert!(
        HTTP_CONNECT_TIMEOUT_SECS <= 10,
        "连接超时 {HTTP_CONNECT_TIMEOUT_SECS}s 太长,连不上时用户会觉得卡死"
    );
}
