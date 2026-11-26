//! Live checks against the hosts in ~/.ssh/config. Run with:
//!   cargo test live -- --ignored --nocapture
//! MYTERM_BUILTIN=1 uses the built-in SSH client (14.4) instead of ssh.

use crate::channel::{self, Channel, Probe, Target, Via};
use crate::{sshconfig, sshlib, tmux};
use std::time::Duration;

async fn via(h: &str) -> Via {
    if std::env::var("MYTERM_BUILTIN").is_ok_and(|v| v == "1") {
        let o = crate::config::load_config().0.hosts.get(h).cloned().unwrap_or_default();
        Via::Builtin(sshlib::resolve(h, o.address.as_deref(), o.user.as_deref(), o.port).await.unwrap())
    } else {
        Via::System(h.to_string())
    }
}

async fn open(h: &str) -> std::sync::Arc<Channel> {
    Channel::spawn(&Target::Remote(via(h).await)).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_hosts() {
    let only = std::env::var("MYTERM_HOST").ok();
    let hosts: Vec<String> = sshconfig::parse()
        .hosts
        .into_iter()
        .filter(|h| only.as_ref().is_none_or(|o| o.split(',').any(|x| x.trim() == h)))
        .collect();
    let mut tasks = vec![];
    for h in hosts {
        tasks.push(tokio::spawn(async move {
            let ch = match Channel::spawn(&Target::Remote(via(&h).await)).await {
                Ok(c) => c,
                Err(e) => return println!("{h}: {e}"),
            };
            let ov = crate::config::load_config().0.hosts.get(&h).and_then(|o| o.tmux_path.clone());
            let r = ch.run(&tmux::bootstrap_script(ov.as_deref()), Duration::from_secs(20)).await;
            let line = match r {
                Ok(r) => {
                    let b = tmux::parse_bootstrap(&r.out);
                    let mut s = format!("{h}: os={} tmux={:?} ver={:?} home={}", b.os, b.tmux, b.version, b.home);
                    if let Some(t) = &b.tmux {
                        // Two requests on the same channel prove framing works.
                        let l = ch.run(&tmux::list_script(t), Duration::from_secs(10)).await.unwrap();
                        match tmux::parse_list(&h, l.code, &l.out) {
                            tmux::ListResult::Sessions(v) => {
                                s += &format!(" sessions={:?}", v.iter().map(|x| (&x.slug, &x.kind, &x.state)).collect::<Vec<_>>())
                            }
                            tmux::ListResult::Error(e) => s += &format!(" list-error={e}"),
                        }
                        let c = ch.run(&tmux::complete_script(&format!("{}/", b.home)), Duration::from_secs(10)).await.unwrap();
                        s += &format!(" completions={}", c.out.lines().count());
                    }
                    s
                }
                Err(e) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let err = ch.stderr_text();
                    let p = match channel::probe(&via(&h).await).await {
                        Probe::Windows { powershell } => format!("WINDOWS powershell={powershell}"),
                        Probe::Other(m) => format!("other: {m}"),
                    };
                    format!("{h}: channel failed ({e}); stderr={:?}; probe={p}", err.trim())
                }
            };
            println!("{line}");
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_rename_window() {
    let h = std::env::var("MYTERM_HOST").unwrap_or("ps-prod".into());
    let ch = open(&h).await;
    let b = tmux::parse_bootstrap(&ch.run(&tmux::bootstrap_script(None), Duration::from_secs(20)).await.unwrap().out);
    let t = b.tmux.unwrap();
    let l = ch.run(&tmux::list_script(&t), Duration::from_secs(10)).await.unwrap();
    let tmux::ListResult::Sessions(v) = tmux::parse_list(&h, l.code, &l.out) else { panic!("list") };
    let s = &v[0];
    let w = &s.windows[0];
    println!("tmux={t} slug={} win={} name={}", s.slug, w.id, w.name);
    let r = ch.run(&tmux::rename_window_script(&t, &s.slug, &w.id, "zz-test"), Duration::from_secs(10)).await.unwrap();
    println!("rename rc={} out={:?}", r.code, r.out);
    let l = ch.run(&tmux::list_script(&t), Duration::from_secs(10)).await.unwrap();
    let tmux::ListResult::Sessions(v) = tmux::parse_list(&h, l.code, &l.out) else { panic!("list") };
    println!("after: {:?}", v[0].windows.iter().map(|x| (&x.id, &x.name)).collect::<Vec<_>>());
    let r = ch.run(&tmux::rename_window_script(&t, &s.slug, &w.id, &w.name), Duration::from_secs(10)).await.unwrap();
    println!("restore rc={}", r.code);
}


/// Uploads a file through each host's channel and reads its checksum back.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_upload() {
    let only = std::env::var("MYTERM_HOST").ok();
    let data: Vec<u8> = (0..2_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let want = data.len();
    for h in sshconfig::parse().hosts {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| x.trim() == h)) {
            continue;
        }
        let ch = open(&h).await;
        let t = std::time::Instant::now();
        let script = crate::files::upload_script("live.bin", &crate::files::base64(&data));
        match ch.run(&script, Duration::from_secs(60)).await {
            Ok(r) if r.code == 0 => {
                let p = r.out.lines().last().unwrap_or("").to_string();
                let s = ch.run(&format!("wc -c < '{p}'; rm -f '{p}'"), Duration::from_secs(10)).await.unwrap();
                let got: usize = s.out.trim().parse().unwrap_or(0);
                println!("{h}: {p} {got}/{want} bytes in {:?} {}", t.elapsed(), if got == want { "OK" } else { "MISMATCH" });
            }
            Ok(r) => println!("{h}: exit {} {}", r.code, r.out.trim()),
            Err(e) => println!("{h}: {e}"),
        }
    }
}

/// A remote PTY through the built-in client: TERM, size, resize, exit code.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_builtin_pty() {
    let h = std::env::var("MYTERM_HOST").unwrap_or("ps-prod".into());
    let o = crate::config::load_config().0.hosts.get(&h).cloned().unwrap_or_default();
    let dest = sshlib::resolve(&h, o.address.as_deref(), o.user.as_deref(), o.port).await.unwrap();
    println!("{dest:?}");
    let cmd = "echo TERM=$TERM; stty size; read x; stty size; exit 7";
    let mut p = sshlib::spawn(&dest, Some(cmd), Some((100, 30))).await.unwrap();
    let mut out = String::new();
    let mut resized = false;
    while let Ok(Some(d)) = tokio::time::timeout(Duration::from_secs(10), p.rx.recv()).await {
        out += &String::from_utf8_lossy(&d);
        if !resized && out.contains("30 100") {
            resized = true;
            p.tx.send(sshlib::In::Resize(120, 40)).unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            p.tx.send(sshlib::In::Data(b"go\r".to_vec())).unwrap();
        }
    }
    let code = p.exit.await.unwrap();
    println!("{out:?} exit={code}");
    assert!(out.contains("TERM=xterm-256color"));
    assert!(out.contains("30 100"));
    assert!(out.contains("40 120"));
    assert_eq!(code, 7);
}
