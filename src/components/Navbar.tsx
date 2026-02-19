import React from "react";

type NavbarProps = {
  isScrolled: boolean;
  onMouseDown: (event: React.MouseEvent<HTMLDivElement>) => void;
  onMinimize: () => void;
};

export default function Navbar({ isScrolled, onMouseDown, onMinimize }: NavbarProps) {
  return (
    <div
      className={`window-navbar ${isScrolled ? "scrolled" : ""}`}
      onMouseDown={onMouseDown}
    >
      <span className="window-title">
        <span className="title-brand-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
            <line x1="8" y1="21" x2="16" y2="21"></line>
            <line x1="12" y1="17" x2="12" y2="21"></line>
          </svg>
        </span>
        Wireless <span className="title-accent">PC</span> Server
      </span>
      <button
        className="window-close-btn"
        onMouseDown={(e) => e.stopPropagation()}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={onMinimize}
        aria-label="Minimize"
        title="Minimize to tray"
      >
        -
      </button>
    </div>
  );
}
