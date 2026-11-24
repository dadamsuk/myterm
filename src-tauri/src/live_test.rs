//! Live checks against the hosts in ~/.ssh/config. Run with:
//!   cargo test live -- --ignored --nocapture

use crate::channel::{self, Channel, Probe, Target};
use crate::{sshconfig, tmux};
use std::time::Duration;

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
            let ch = Channel::spawn(&Target::Ssh(h.clone())).unwrap();
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
                    let p = match channel::probe(&h).await {
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
    let ch = Channel::spawn(&Target::Ssh(h.clone())).unwrap();
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
        let ch = Channel::spawn(&Target::Ssh(h.clone())).unwrap();
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
