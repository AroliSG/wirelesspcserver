
type GeneralSectionProps = {
  loading: boolean;
  error: string;
  info: { host: string; port: number; token: string; device_name: string; device_model: string } | null;
  connectedTo: string;
  copiedCode: boolean;
  onShowQr: () => void;
  onOpenDevices: () => void;
  onCopyCode: () => void;
};

export default function GeneralSection({
  loading,
  error,
  info,
  connectedTo,
  copiedCode,
  onShowQr,
  onOpenDevices,
  onCopyCode,
}: GeneralSectionProps) {
  return (
    <div className="content-block block-blue">
      {loading ? <p className="state-msg">Generating pairing info...</p> : null}
      {error ? <p className="state-msg error">{error}</p> : null}

      <p className="section-label">General</p>
      <div className="pref-card tone-dark">
        <div>
          <p className="pref-title">This Computer</p>
          <p className="pref-sub">Host and listening port: {info ? `${info.host}:${info.port}` : "Loading..."}</p>
          <p className="pref-main">{info ? `${info.device_name} (${info.device_model})` : "Loading..."}</p>
        </div>
        <button onClick={onShowQr}>Show QR</button>
      </div>

      <div className="pref-card tone-blue">
        <div>
          <p className="pref-title">Connected Devices</p>
          <p className="pref-sub">{connectedTo ? `Connected to ${connectedTo}` : "No active device"}</p>
        </div>
        <button className="secondary" onClick={onOpenDevices}>View</button>
      </div>

      <div className="pref-card tone-dark">
        <div>
          <p className="pref-title">Quick Code</p>
          <p className="pref-sub">Use this in your phone app</p>
          <p className="pref-main mono">{info?.token || "----"}</p>
        </div>
        <button onClick={onCopyCode}>{copiedCode ? "Copied" : "Copy"}</button>
      </div>
    </div>
  );
}
