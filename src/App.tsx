import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";
import { QRCodeSVG } from "qrcode.react";
import { createPortal } from "react-dom";
import Navbar from "./components/Navbar";
import GeneralSection from "./components/sections/GeneralSection";
import SecuritySection from "./components/sections/SecuritySection";
import SettingsSection from "./components/sections/SettingsSection";
import "./App.css";

const UPDATES_URL = "https://wirelesspc.arolisg.dev";
const OPEN_APPS_MAX = 6;

interface PairingInfo {
  host: string;
  port: number;
  token: string;
  ws_url: string;
  device_name: string;
  device_model: string;
}

interface ConnectedDeviceEntry {
  id: string;
  name: string;
  model: string;
  ip: string;
  times: number;
  secured: boolean;
  password_required: boolean;
  active_sessions: number;
}

interface ConnectionEvent {
  connections: number;
  connected_to: string;
  devices: ConnectedDeviceEntry[];
}

interface OpenAppItem {
  id: string;
  name: string;
  path: string;
  icon_data_url?: string | null;
}

type UpdaterState = "idle" | "checking" | "up-to-date" | "downloading" | "error";

const compactPath = (fullPath: string) => {
  const normalized = fullPath.replace(/\//g, "\\");
  const parts = normalized.split("\\").filter(Boolean);
  if (parts.length <= 2) return normalized;
  return `...\\${parts[parts.length - 2]}\\${parts[parts.length - 1]}`;
};

function App() {
  const scrollRef = useRef<HTMLElement | null>(null);
  const [isScrolled, setIsScrolled] = useState(false);
  const [info, setInfo] = useState<PairingInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [powerActionsEnabled, setPowerActionsEnabled] = useState(false);
  const [connectionCodeEnabled, setConnectionCodeEnabled] = useState(false);
  const [launchOnStartup, setLaunchOnStartup] = useState(false);
  const [qrOpen, setQrOpen] = useState(false);
  const [copiedCode, setCopiedCode] = useState(false);
  const [copiedIp, setCopiedIp] = useState(false);
  const [connectedTo, setConnectedTo] = useState("");
  const [connectedDevices, setConnectedDevices] = useState<ConnectedDeviceEntry[]>([]);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [clearingHistory, setClearingHistory] = useState(false);
  const [openApps, setOpenApps] = useState<OpenAppItem[]>([]);
  const [appPathDraft, setAppPathDraft] = useState("");
  const [appsBusy, setAppsBusy] = useState(false);
  const [appsOpen, setAppsOpen] = useState(false);
  const [updaterOpen, setUpdaterOpen] = useState(false);
  const [updaterState, setUpdaterState] = useState<UpdaterState>("idle");
  const [updaterMessage, setUpdaterMessage] = useState("");
  const [currentVersion, setCurrentVersion] = useState("");
  const appsAtLimit = openApps.length >= OPEN_APPS_MAX;

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

  const loadSettings = async () => {
    try {
      const [power, code, startup] = await Promise.all([
        invoke<boolean>("get_power_actions_enabled"),
        invoke<boolean>("get_connection_code_enabled"),
        invoke<boolean>("get_launch_on_startup"),
      ]);
      setPowerActionsEnabled(power);
      setConnectionCodeEnabled(code);
      setLaunchOnStartup(startup);
    } catch {
      setPowerActionsEnabled(false);
      setConnectionCodeEnabled(false);
      setLaunchOnStartup(false);
    }
  };

  const loadOpenApps = async () => {
    try {
      const payload = await invoke<{ items: OpenAppItem[] }>("get_open_apps");
      setOpenApps(payload.items || []);
    } catch {
      setOpenApps([]);
    }
  };

  const togglePowerActions = async () => {
    const next = !powerActionsEnabled;
    try {
      await invoke("set_power_actions_enabled", { enabled: next });
      setPowerActionsEnabled(next);
    } catch {
      // keep current value on failure
    }
  };

  const toggleConnectionCode = async () => {
    const next = !connectionCodeEnabled;
    try {
      await invoke("set_connection_code_enabled", { enabled: next });
      setConnectionCodeEnabled(next);
    } catch {
      // keep current value on failure
    }
  };

  const toggleLaunchOnStartup = async () => {
    const next = !launchOnStartup;
    try {
      const ok = await invoke<boolean>("set_launch_on_startup", { enabled: next });
      if (ok) setLaunchOnStartup(next);
    } catch {
      // keep current value on failure
    }
  };

  const copyCode = async () => {
    if (!info) return;
    try {
      await navigator.clipboard.writeText(info.token);
      setCopiedCode(true);
      setTimeout(() => setCopiedCode(false), 1200);
    } catch {}
  };

  const copyIp = async () => {
    if (!info) return;
    try {
      await navigator.clipboard.writeText(info.host);
      setCopiedIp(true);
      setTimeout(() => setCopiedIp(false), 1200);
    } catch {}
  };

  const handleTitlebarMouseDown = async (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest("button")) return;
    try {
      await getCurrentWindow().startDragging();
    } catch {
      // ignore
    }
  };

  const handleMinimizeToTray = async () => {
    try {
      await getCurrentWindow().hide();
    } catch {
      try {
        await getCurrentWindow().minimize();
      } catch {
        // ignore
      }
    }
  };

  const clearDeviceHistory = async () => {
    if (clearingHistory) return;
    setClearingHistory(true);
    try {
      await invoke("clear_connected_devices_history");
      setConnectedDevices([]);
      setConnectedTo("");
    } finally {
      setClearingHistory(false);
    }
  };

  const addOpenApp = async () => {
    const path = appPathDraft.trim();
    if (!path || appsBusy || appsAtLimit) return;
    setAppsBusy(true);
    try {
      await invoke("add_open_app", { path });
      setAppPathDraft("");
      await loadOpenApps();
    } finally {
      setAppsBusy(false);
    }
  };

  const pickOpenApp = async () => {
    if (appsBusy || appsAtLimit) return;
    setAppsBusy(true);
    try {
      const path = await invoke<string | null>("pick_open_app_path");
      if (!path) return;
      await invoke("add_open_app", { path });
      await loadOpenApps();
    } finally {
      setAppsBusy(false);
    }
  };

  const discoverOpenApps = async () => {
    if (appsBusy || appsAtLimit) return;
    setAppsBusy(true);
    try {
      await invoke("discover_open_apps");
      await loadOpenApps();
    } finally {
      setAppsBusy(false);
    }
  };

  const removeOpenApp = async (id: string) => {
    if (appsBusy) return;
    setAppsBusy(true);
    try {
      await invoke("remove_open_app", { id });
      await loadOpenApps();
    } finally {
      setAppsBusy(false);
    }
  };

  const checkForUpdates = async () => {
    setUpdaterOpen(true);
    setUpdaterState("checking");
    setUpdaterMessage("Searching for updates...");
    try {
      const update = await check();
      if (update) {
        setUpdaterState("downloading");
        setUpdaterMessage("Update found. Downloading and installing...");
        await update.downloadAndInstall();
        setUpdaterMessage("Update installed. Restarting app...");
        return;
      }
      setUpdaterState("up-to-date");
      setUpdaterMessage(
        `You are using the latest version${currentVersion ? ` (v${currentVersion})` : ""}.`
      );
    } catch {
      setUpdaterState("error");
      setUpdaterMessage("Could not check updates right now.");
    }
  };

  useEffect(() => {
    void refreshPairing();
    void loadSettings();
    void loadOpenApps();
    void getVersion()
      .then((v) => setCurrentVersion(v))
      .catch(() => setCurrentVersion(""));

    let unlistenConn: (() => void) | null = null;
    let unlistenOpenQr: (() => void) | null = null;
    let unlistenOpenPref: (() => void) | null = null;
    let unlistenOpenApps: (() => void) | null = null;
    let unlistenCheckUpdates: (() => void) | null = null;

    const setup = async () => {
      unlistenConn = await listen<ConnectionEvent>("ws-connection-state", (event) => {
        setConnectedTo(event.payload.connected_to || "");
        setConnectedDevices(event.payload.devices || []);
      });

      unlistenOpenQr = await listen("open-qr", () => {
        setQrOpen(true);
      });

      unlistenOpenPref = await listen("open-preferences", () => {
        setQrOpen(false);
        setAppsOpen(false);
      });

      unlistenOpenApps = await listen("open-apps", () => {
        setQrOpen(false);
        setDevicesOpen(false);
        setAppsOpen(true);
      });

      unlistenCheckUpdates = await listen("check-updates", () => {
        void checkForUpdates();
      });
    };

    void setup();

    return () => {
      if (unlistenConn) unlistenConn();
      if (unlistenOpenQr) unlistenOpenQr();
      if (unlistenOpenPref) unlistenOpenPref();
      if (unlistenOpenApps) unlistenOpenApps();
      if (unlistenCheckUpdates) unlistenCheckUpdates();
    };
  }, []);

  useEffect(() => {
    const node = scrollRef.current;
    if (!node) return;

    const onScroll = () => {
      setIsScrolled(node.scrollTop > 4);
    };

    onScroll();
    node.addEventListener("scroll", onScroll);
    return () => node.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <section ref={scrollRef} className="prefs-window">
      <Navbar
        isScrolled={isScrolled}
        onMouseDown={(e) => void handleTitlebarMouseDown(e)}
        onMinimize={() => {
          void handleMinimizeToTray();
        }}
      />

      <GeneralSection
        loading={loading}
        error={error}
        info={info}
        connectedTo={connectedTo}
        copiedCode={copiedCode}
        onShowQr={() => setQrOpen(true)}
        onOpenDevices={() => setDevicesOpen(true)}
        onCopyCode={() => {
          void copyCode();
        }}
      />

      <SecuritySection
        connectionCodeEnabled={connectionCodeEnabled}
        powerActionsEnabled={powerActionsEnabled}
        onToggleConnectionCode={() => {
          void toggleConnectionCode();
        }}
        onTogglePowerActions={() => {
          void togglePowerActions();
        }}
      />

      <SettingsSection
        launchOnStartup={launchOnStartup}
        onToggleLaunchOnStartup={() => {
          void toggleLaunchOnStartup();
        }}
      />

      {qrOpen && info && (
        createPortal(
          <div className="modal-overlay" onClick={() => setQrOpen(false)}>
            <div className="qr-modal" onClick={(e) => e.stopPropagation()}>
              <h3>IP & QR Code</h3>
              <div className="qr-ip-row">
                <span>{info.host}</span>
                <button className="secondary" onClick={copyIp}>{copiedIp ? "Copied" : "Copy"}</button>
              </div>
              <div className="qr-box">
                <QRCodeSVG value={info.ws_url} size={150} bgColor="#ffffff" fgColor="#0b1220" level="M" includeMargin />
              </div>
              <p className="qr-help">Scan from your phone or enter host + quick code manually.</p>
              <button className="secondary full" onClick={() => setQrOpen(false)}>Done</button>
            </div>
          </div>,
          document.body
        )
      )}

      {devicesOpen && (
        createPortal(
          <div className="modal-overlay" onClick={() => setDevicesOpen(false)}>
            <div className="qr-modal" onClick={(e) => e.stopPropagation()}>
              <h3>Connected Devices</h3>
              <div className="devices-list">
                {connectedDevices.length === 0 ? (
                  <p className="qr-help">No devices connected yet.</p>
                ) : (
                  connectedDevices.map((d) => (
                    <div key={d.id} className="device-row">
                      <div className="device-row-head">
                        <strong>{d.name}</strong>
                        <span>{d.active_sessions > 0 ? `Online (${d.active_sessions})` : "Offline"}</span>
                      </div>
                      <p>
                        {d.model} | Times {d.times} | IP {d.ip}
                      </p>
                      <p>
                        Secured {d.secured ? "Yes" : "No"} | Password {d.password_required ? "Yes" : "No"}
                      </p>
                    </div>
                  ))
                )}
              </div>
              <button className="secondary full" onClick={() => void clearDeviceHistory()} disabled={clearingHistory}>
                {clearingHistory ? "Clearing..." : "Clear History"}
              </button>
              <button className="secondary full" onClick={() => setDevicesOpen(false)}>Done</button>
            </div>
          </div>,
          document.body
        )
      )}

      {appsOpen && (
        createPortal(
          <div className="modal-overlay" onClick={() => setAppsOpen(false)}>
            <div className="apps-modal" onClick={(e) => e.stopPropagation()}>
              <div className="apps-head">
                <div>
                  <h3>Apps</h3>
                  <p className="qr-help">Manage .exe apps for mobile Open Apps</p>
                </div>
                <span className="pref-badge">{openApps.length}/{OPEN_APPS_MAX}</span>
              </div>
              {appsAtLimit ? <p className="qr-help">Limit reached (6 apps max). Remove one to add another.</p> : null}

              <div className="app-row">
                <input
                  className="app-input"
                  placeholder="C:\\Path\\App.exe"
                  value={appPathDraft}
                  onChange={(e) => setAppPathDraft(e.target.value)}
                />
                <button onClick={() => void addOpenApp()} disabled={appsBusy || appsAtLimit}>Add</button>
              </div>

              <div className="app-actions">
                <button className="secondary" onClick={() => void pickOpenApp()} disabled={appsBusy || appsAtLimit}>Select .exe</button>
                <button className="secondary" onClick={() => void discoverOpenApps()} disabled={appsBusy || appsAtLimit}>Auto Discover</button>
              </div>

              <div className="apps-list">
                {openApps.length === 0 ? (
                  <div className="apps-empty">
                    <p className="qr-help">No apps configured yet.</p>
                  </div>
                ) : (
                  openApps.map((app) => (
                    <div key={app.id} className="app-item">
                      <div className="app-left">
                        {app.icon_data_url ? (
                          <img className="app-icon" src={app.icon_data_url} alt={app.name} />
                        ) : (
                          <div className="app-icon app-icon-fallback">.exe</div>
                        )}
                        <div className="app-copy">
                          <p className="pref-title">{app.name}</p>
                          <p className="pref-sub">{compactPath(app.path)}</p>
                        </div>
                      </div>
                      <button className="secondary app-remove-btn" onClick={() => void removeOpenApp(app.id)} disabled={appsBusy}>Remove</button>
                    </div>
                  ))
                )}
              </div>
              <button className="secondary full" onClick={() => setAppsOpen(false)}>Done</button>
            </div>
          </div>,
          document.body
        )
      )}

      {updaterOpen && (
        createPortal(
          <div
            className="modal-overlay"
            onClick={() => {
              if (updaterState !== "checking" && updaterState !== "downloading") {
                setUpdaterOpen(false);
              }
            }}
          >
            <div className="qr-modal updater-modal" onClick={(e) => e.stopPropagation()}>
              <h3>Update</h3>
              {(updaterState === "checking" || updaterState === "downloading") ? (
                <div className="updater-progress" aria-live="polite">
                  <span className="updater-spinner" aria-hidden="true"></span>
                  <p className="qr-help">{updaterMessage}</p>
                </div>
              ) : (
                <p className="qr-help" aria-live="polite">{updaterMessage}</p>
              )}
              {updaterState === "error" ? (
                <button className="secondary full" onClick={() => window.open(UPDATES_URL, "_blank", "noopener,noreferrer")}>
                  Open Downloads Page
                </button>
              ) : null}
              <button
                className="secondary full"
                onClick={() => setUpdaterOpen(false)}
                disabled={updaterState === "checking" || updaterState === "downloading"}
              >
                Close
              </button>
            </div>
          </div>,
          document.body
        )
      )}
    </section>
  );
}

export default App;

