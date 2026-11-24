"""Compute delivery metrics from subscriber application timestamps."""
import json
from pathlib import Path

import numpy as np
import pandas as pd


def summarize(path, expected, interval_ms):
    data = pd.read_json(path, lines=True)
    if data.empty:
        raise ValueError("No observations")
    columns = ["sequence","sent_us","received_us","elapsed_ms","bytes"]
    if set(data.columns) != set(columns) or not np.isfinite(data[columns]).all().all():
        raise ValueError("Malformed measurement columns")
    if data.sequence.duplicated().any() or not data.sequence.between(0,expected-1).all():
        raise ValueError("Duplicate or invalid object sequence")
    latency = (data.received_us-data.sent_us)/1000
    if (latency < 0).any():
        raise ValueError("Unsynchronized clocks")
    arrivals = data.sort_values("received_us")
    duration_s = (arrivals.received_us.max()-arrivals.received_us.min())/1e6
    gaps_ms = arrivals.received_us.diff().dropna()/1000
    return {"delivered":len(data),"expected":expected,"delivery_ratio":len(data)/expected,
            "latency_p50_ms":float(latency.median()),"latency_p95_ms":float(latency.quantile(.95)),
            "startup_ms":float(data.elapsed_ms.iloc[0]),
            "payload_mbps":float(arrivals.bytes.iloc[1:].sum()*8/max(duration_s,1e-9)/1e6),
            "interruption_gaps":int((gaps_ms > max(100,3*interval_ms)).sum()),
            "max_gap_ms":float(gaps_ms.max()) if len(gaps_ms) else 0}


if __name__ == "__main__":
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    data = pd.read_json("results/summary.json")
    averages = data.groupby(["profile","subscribers"])[["latency_p95_ms","startup_ms","payload_mbps","delivery_ratio"]].mean()
    averages.to_csv("results/averages.csv")
    fig, axes = plt.subplots(1,2,figsize=(10,4),layout="constrained")
    averages.latency_p95_ms.unstack().plot.bar(ax=axes[0],rot=0,ylabel="Mean client p95 latency (ms)")
    averages.delivery_ratio.unstack().plot.bar(ax=axes[1],rot=0,ylim=(0,1.05),ylabel="Mean delivery ratio")
    fig.savefig("results/delivery.png",dpi=160)
    print(averages.to_string())
