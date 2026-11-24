"""Run bounded FFmpeg/JPEG delivery over localhost QUIC, optionally with Linux netem."""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

from analyze import summarize

COUNT = 90
INTERVAL = 33
PROFILES = {"clean": [], "delay": ["delay","25ms","5ms"], "loss": ["delay","15ms","3ms","loss","1%"]}


def check(command):
    subprocess.run(command, check=True, timeout=120, stdout=subprocess.DEVNULL)


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def prepare(work):
    work.mkdir(parents=True, exist_ok=True)
    frames = work / "frames"
    frames.mkdir(exist_ok=True)
    check(["ffmpeg","-hide_banner","-loglevel","error","-f","lavfi","-i",
           "testsrc2=size=160x90:rate=30","-frames:v","30","-q:v","5","-threads","1","-y",str(frames / "%03d.jpg")])
    key, cert = work / "server.key", work / "server.pem"
    ca = work / "ca.pem"
    if not cert.exists():
        ca_key, csr, extensions = work/"ca.key", work/"server.csr", work/"leaf.ext"
        check(["openssl","req","-x509","-newkey","rsa:2048","-nodes","-keyout",str(ca_key),
               "-out",str(ca),"-days","2","-subj","/CN=MoQStreamLab local CA",
               "-addext","basicConstraints=critical,CA:TRUE"])
        check(["openssl","req","-newkey","rsa:2048","-nodes","-keyout",str(key),
               "-out",str(csr),"-subj","/CN=localhost"])
        extensions.write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n"
                              "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
        check(["openssl","x509","-req","-in",str(csr),"-CA",str(ca),"-CAkey",str(ca_key),
               "-CAcreateserial","-out",str(cert),"-days","2","-extfile",str(extensions)])
        key.chmod(0o600)
        ca_key.chmod(0o600)
    return frames, cert, key, ca


def run_profile(binary, work, profile, subscribers, repeat, impair):
    folder = work / f"{profile}-{subscribers}-{repeat}"
    folder.mkdir()  # Refuse to mix measurements with an earlier run.
    if impair and PROFILES[profile]:
        check(["sudo","tc","qdisc","replace","dev","lo","root","netem",*PROFILES[profile],"seed",str(17+repeat)])
    pub = None
    clients = []
    handles = []
    try:
        frames, cert, key, ca = prepare(work)
        stderr = (folder / "publisher.err").open("w")
        handles.append(stderr)
        pub = subprocess.Popen([str(binary),"publish","--frames",str(frames),"--cert",str(cert),"--key",str(key),
                                "--count",str(COUNT),"--interval-ms",str(INTERVAL)], stdout=subprocess.PIPE,
                               stderr=stderr,text=True)
        # Linux readiness wait stays bounded even if startup fails before the first line.
        ready, _, _ = select.select([pub.stdout], [], [], 8)
        if not ready or pub.stdout.readline().strip() != '{"event":"ready"}':
            raise RuntimeError("Publisher did not become ready")
        for client in range(subscribers):
            handle = (folder / f"subscriber-{client}.err").open("w")
            handles.append(handle)
            clients.append(subprocess.Popen([str(binary),"subscribe","--ca",str(ca),"--out",
                str(folder / f"subscriber-{client}.jsonl"),"--count",str(COUNT)], stderr=handle,
                stdout=subprocess.DEVNULL))
        statuses = [proc.wait(timeout=20) for proc in clients]
        if pub.wait(timeout=20) != 0:
            raise RuntimeError("Publisher failed")
        reports = []
        for index, status in enumerate(statuses):
            path = folder / f"subscriber-{index}.jsonl"
            if not path.exists():
                raise RuntimeError("Subscriber did not create a log")
            report = summarize(path, COUNT, INTERVAL)
            if status and "Subscriber timed out" not in (folder / f"subscriber-{index}.err").read_text():
                raise RuntimeError("Subscriber failed for a reason other than a delivery timeout")
            if profile == "clean" and report["delivered"] != COUNT:
                raise RuntimeError("Clean profile lost an object")
            reports.append({"profile":profile,"subscribers":subscribers,"repeat":repeat,"client":index,
                            "exit_code":status,**report})
        return reports
    finally:
        for proc in clients:
            stop(proc)
        if pub is not None:
            stop(pub)
        for handle in handles:
            handle.close()
        if impair and PROFILES[profile]:
            subprocess.run(["sudo","tc","qdisc","del","dev","lo","root"],check=False,
                           stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/moqstreamlab"))
    parser.add_argument("--work", type=Path, default=Path("work"))
    parser.add_argument("--netem",action="store_true",help="Modify loopback qdisc on an isolated Linux runner")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("The bounded process runner requires Linux")
    for program in ("ffmpeg","openssl"):
        if not shutil.which(program):
            parser.error(program + " is required")
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error("Compile the binary first")
    work = args.work.resolve()
    prepare(work)
    profiles = PROFILES if args.netem else {"clean":[]}
    rows = [row for profile in profiles for subs in (1,2) for repeat in range(3)
            for row in run_profile(binary,work,profile,subs,repeat,args.netem)]
    Path("results").mkdir(exist_ok=True)
    Path("results/summary.json").write_text(json.dumps(rows,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(rows,indent=2))


if __name__ == "__main__":
    main()
