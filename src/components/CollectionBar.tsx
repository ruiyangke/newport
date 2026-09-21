import { RefreshButton } from "./controls";

export function CollectionBar({
  busy,
  updated,
  error,
  refresh,
  children,
  sampled = false,
}: {
  busy: boolean;
  updated: Date | null;
  error: string;
  refresh: () => void;
  children?: React.ReactNode;
  sampled?: boolean;
}) {
  return (
    <>
      <div className="collection-bar">
        <span className="muted">
          {busy
            ? "Reading server…"
            : updated
              ? `${error ? "Last success" : sampled ? "Sampled" : "Updated"} ${updated.toLocaleTimeString()}`
              : "Not loaded"}
        </span>
        <div className="section-actions">
          {children}
          <RefreshButton busy={busy} onClick={refresh} />
        </div>
      </div>
      {error && (
        <p className="cockpit-error" role="alert">
          {error}
          {updated && " Showing the last available reading."}
        </p>
      )}
    </>
  );
}
