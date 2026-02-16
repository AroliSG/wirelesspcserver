import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface PairingInfo {
  host: string;
  port: number;
  token: string;
  ws_url: string;
}

function App() {
  const [info, setInfo] = useState<PairingInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  const refreshPairing = async () => {
    setLoading(true);
    setError("");
    try {
      const next = await invoke<PairingInfo>("generate_pairing_info");
      setInfo(next);
    } catch {
      setError("Could not generate pairing info.");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refreshPairing();
  }, []);

  const copyUrl = async () => {
    if (!info) return;
    await navigator.clipboard.writeText(info.ws_url);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <main className="page">
      <section className="card">
        <h1>MousePilot Host</h1>
        <p className="muted">
          Desktop host is ready. Mobile app will connect using this pairing URL.
        </p>

        {loading ? (
          <p className="muted">Generating pairing info...</p>
        ) : error ? (
          <p className="error">{error}</p>
        ) : info ? (
          <div className="stack">
            <div className="row">
              <span className="label">Host</span>
              <span className="value">{info.host}</span>
            </div>
            <div className="row">
              <span className="label">Port</span>
              <span className="value">{info.port}</span>
            </div>
            <div className="row">
              <span className="label">Token</span>
              <span className="value mono">{info.token}</span>
            </div>
            <div className="url-box mono">{info.ws_url}</div>
            <div className="actions">
              <button onClick={copyUrl}>{copied ? "Copied" : "Copy URL"}</button>
              <button onClick={() => void refreshPairing()}>Regenerate</button>
            </div>
            <p className="hint">
              Next: open mobile app, paste this URL, and connect over the same LAN.
            </p>
          </div>
        ) : null}
      </section>
    </main>
  );
}

export default App;
