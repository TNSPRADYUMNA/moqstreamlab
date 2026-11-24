use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use moqstreamlab::{decode, encode, HEADER, MAX_IMAGE};
use std::{collections::HashSet, io::Write, net::SocketAddr, path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tokio::{io::AsyncWriteExt, task::JoinSet, time::timeout};
use url::Url;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    Publish {
        #[arg(long, default_value = "127.0.0.1:4433")]
        bind: SocketAddr,
        #[arg(long)]
        cert: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        frames: PathBuf,
        #[arg(long, default_value_t = 120)]
        count: u64,
        #[arg(long, default_value_t = 33)]
        interval_ms: u64,
        #[arg(long, default_value_t = 2500)]
        lead_ms: u64,
        #[arg(long, default_value = "video")]
        track: String,
    },
    Subscribe {
        #[arg(long, default_value = "https://localhost:4433/")]
        url: Url,
        #[arg(long)]
        ca: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 120)]
        count: u64,
        #[arg(long, default_value_t = 12)]
        timeout_seconds: u64,
        #[arg(long, default_value = "video")]
        track: String,
    },
}

fn now_us() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros().try_into()?)
}

fn valid_track(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().mode {
        Mode::Publish { bind, cert, key, frames, count, interval_ms, lead_ms, track } => {
            ensure!(valid_track(&track) && (1..=1800).contains(&count) && (10..=1000).contains(&interval_ms)
                    && (500..=10000).contains(&lead_ms), "Invalid publisher bounds");
            ensure!(bind.ip().is_loopback() || bind.ip().is_unspecified(), "Bind locally or inside an isolated container");
            let mut paths: Vec<_> = std::fs::read_dir(frames)?.map(|p| p.map(|p| p.path()))
                .collect::<std::io::Result<_>>()?;
            paths.retain(|p| p.extension().is_some_and(|e| e == "jpg"));
            paths.sort();
            ensure!(!paths.is_empty() && paths.len() <= 300, "Expected 1–300 JPEG files");
            let mut images = Vec::new();
            for path in paths {
                ensure!(std::fs::metadata(&path)?.len() <= MAX_IMAGE as u64, "Oversized image");
                let data = std::fs::read(path)?;
                decode(&encode(0,0,&data)?)?;
                images.push(data);
            }
            let origin = moq_lite::Origin::produce();
            let mut broadcast = moq_lite::Broadcast::produce();
            let mut producer = broadcast.create_track(moq_lite::Track::new(track));
            origin.publish_broadcast("media", broadcast.consume());
            let mut config = moq_native::ServerConfig::default();
            config.bind = Some(bind);
            config.tls.cert = vec![cert];
            config.tls.key = vec![key];
            let mut server = config.init()?.with_publish(origin.consume());
            println!("{{\"event\":\"ready\"}}");
            std::io::stdout().flush()?;
            let publish = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(lead_ms)).await;
                for sequence in 0..count {
                    let frame = encode(sequence, now_us()?, &images[sequence as usize % images.len()])?;
                    let mut group = producer.append_group();
                    group.write_frame(bytes::Bytes::from(frame));
                    group.close();
                    tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                drop(producer);
                Ok::<_, anyhow::Error>(())
            });
            tokio::pin!(publish);
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    result = &mut publish => { result??; break; }
                    request = server.accept() => {
                        let request = request.context("Server closed")?;
                        if sessions.len() >= 8 {
                            request.close(429).await?;
                        } else {
                            sessions.spawn(async move {
                                let session = timeout(Duration::from_secs(4), request.ok()).await??;
                                let _ = session.closed().await;
                                Ok::<_, anyhow::Error>(())
                            });
                        }
                    }
                    Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
                }
            }
            sessions.abort_all();
            while sessions.join_next().await.is_some() {}
        }
        Mode::Subscribe { url, ca, out, count, timeout_seconds, track } => {
            ensure!(url.scheme() == "https", "Use HTTPS with a trusted certificate");
            ensure!(valid_track(&track) && (1..=1800).contains(&count) && (1..=60).contains(&timeout_seconds),
                    "Invalid subscriber bounds");
            let started = Instant::now();
            let mut config = moq_native::ClientConfig::default();
            config.tls.root = vec![ca];
            let origin = moq_lite::Origin::produce();
            let mut consumer = origin.consume();
            let client = config.init()?.with_consume(origin);
            let session = timeout(Duration::from_secs(5), client.connect(url)).await??;
            let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(out).await?;
            let receive = async {
                let broadcast = loop {
                    let (path, available) = consumer.announced().await.context("Origin ended")?;
                    if path.to_string() == "media" {
                        if let Some(broadcast) = available { break broadcast; }
                    }
                };
                let mut reader = broadcast.subscribe_track(&moq_lite::Track::new(track));
                let mut seen = HashSet::new();
                while let Some(mut group) = reader.next_group().await? {
                    while let Some(mut frame) = group.next_frame().await? {
                        ensure!(frame.info.size <= (MAX_IMAGE + HEADER) as u64, "Frame exceeds limit");
                        let bytes = frame.read_all().await?;
                        let (sequence, sent_us, image) = decode(&bytes)?;
                        ensure!(sequence < count && seen.insert(sequence), "Duplicate or out-of-range object");
                        let received_us = now_us()?;
                        ensure!(received_us >= sent_us, "Sender and receiver clocks disagree");
                        let row = serde_json::json!({"sequence":sequence,"sent_us":sent_us,"received_us":received_us,
                            "elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"bytes":image.len()});
                        file.write_all(format!("{row}\n").as_bytes()).await?;
                        if seen.len() == count as usize { return Ok::<_, anyhow::Error>(seen.len()); }
                    }
                }
                Ok(seen.len())
            };
            let outcome = timeout(Duration::from_secs(timeout_seconds), receive).await;
            file.flush().await?;
            drop(session);
            match outcome {
                Ok(result) => { let got = result?; ensure!(got > 0, "No objects received"); }
                Err(_) => anyhow::bail!("Subscriber timed out; partial log retained"),
            }
        }
    }
    Ok(())
}
