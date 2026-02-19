
type SecuritySectionProps = {
  connectionCodeEnabled: boolean;
  powerActionsEnabled: boolean;
  onToggleConnectionCode: () => void;
  onTogglePowerActions: () => void;
};

export default function SecuritySection({
  connectionCodeEnabled,
  powerActionsEnabled,
  onToggleConnectionCode,
  onTogglePowerActions,
}: SecuritySectionProps) {
  return (
    <div className="content-block block-dark">
      <p className="section-label">Security</p>
      <div className="pref-card tone-blue">
        <div>
          <p className="pref-title">Connection Code</p>
          <p className="pref-sub">Require quick code to connect (default: Off)</p>
        </div>
        <button className={`toggle ${connectionCodeEnabled ? "on" : "off"}`} onClick={onToggleConnectionCode}>
          {connectionCodeEnabled ? "On" : "Off"}
        </button>
      </div>

      <div className="pref-card tone-dark">
        <div>
          <p className="pref-title">Power Actions</p>
          <p className="pref-sub">Allow sleep/lock/restart/shutdown from mobile</p>
        </div>
        <button className={`toggle ${powerActionsEnabled ? "on" : "off"}`} onClick={onTogglePowerActions}>
          {powerActionsEnabled ? "On" : "Off"}
        </button>
      </div>
    </div>
  );
}
