
type SettingsSectionProps = {
  launchOnStartup: boolean;
  onToggleLaunchOnStartup: () => void;
};

export default function SettingsSection({ launchOnStartup, onToggleLaunchOnStartup }: SettingsSectionProps) {
  return (
    <div className="content-block block-dark">
      <p className="section-label">Settings</p>
      <div className="pref-card tone-blue">
        <div>
          <p className="pref-title">Launch on Startup</p>
          <p className="pref-sub">Start Wireless PC Server when Windows starts</p>
        </div>
        <button className={`toggle ${launchOnStartup ? "on" : "off"}`} onClick={onToggleLaunchOnStartup}>
          {launchOnStartup ? "On" : "Off"}
        </button>
      </div>
    </div>
  );
}
