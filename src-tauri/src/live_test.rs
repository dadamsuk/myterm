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
        .filter(|h| only.as_ref().is_none_or(|o| o == h))
        .collect();
    let mut tasks = vec![];
    for h in hosts {
        tasks.push(tokio::spawn(async move {
            let ch = Channel::spawn(&Target::Ssh(h.clone())).unwrap();
            let r = ch.run(&tmux::bootstrap_script(None), Duration::from_secs(20)).await;
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
