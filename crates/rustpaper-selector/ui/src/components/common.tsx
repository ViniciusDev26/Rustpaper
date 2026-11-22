import { useEffect, useRef, useState, type ReactNode } from "react";
import { useToast } from "../store/toast";

const BADGE: Record<string, string> = {
  scene: "bg-accent text-ink",
  video: "bg-green text-ink",
  web: "bg-yellow text-ink",
  questionable: "bg-red text-ink",
  mature: "bg-red text-ink",
};

export function Badge({ children }: { children: string }) {
  return (
    <span
      className={`inline-block rounded px-1.5 py-0.5 text-[0.65rem] uppercase tracking-wide ${BADGE[children] ?? "bg-surface text-fg"}`}
    >
      {children}
    </span>
  );
}

type Variant = "primary" | "secondary" | "danger" | "confirm" | "tab" | "tab-active";
const VARIANT: Record<Variant, string> = {
  primary: "bg-accent text-ink font-semibold",
  secondary: "bg-edge text-fg font-semibold",
  danger: "bg-edge text-red font-semibold",
  confirm: "bg-red text-ink font-semibold",
  tab: "bg-transparent text-sub",
  "tab-active": "bg-card text-fg",
};

export function Button({
  variant = "primary",
  className = "",
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      {...props}
      className={`cursor-pointer rounded-md px-3 py-1.5 text-sm disabled:cursor-default disabled:opacity-50 ${VARIANT[variant]} ${className}`}
    />
  );
}

/// Destructive action in two clicks: the first arms it for 4 s.
export function ConfirmButton({
  label,
  confirmLabel = "Click to confirm",
  busyLabel,
  onConfirm,
  className = "",
}: {
  label: string;
  confirmLabel?: string;
  busyLabel: string;
  onConfirm: () => Promise<unknown>;
  className?: string;
}) {
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const click = async () => {
    if (!armed) {
      setArmed(true);
      timer.current = setTimeout(() => setArmed(false), 4000);
      return;
    }
    clearTimeout(timer.current);
    setBusy(true);
    try {
      await onConfirm();
    } finally {
      setBusy(false);
      setArmed(false);
    }
  };
  return (
    <Button variant={armed ? "confirm" : "danger"} disabled={busy} onClick={click} className={className}>
      {busy ? busyLabel : armed ? confirmLabel : label}
    </Button>
  );
}

export function Toast() {
  const message = useToast((s) => s.message);
  return (
    <div
      className={`pointer-events-none fixed bottom-4 left-1/2 -translate-x-1/2 rounded-lg bg-accent px-4 py-2 text-sm text-ink transition-opacity ${message ? "opacity-100" : "opacity-0"}`}
    >
      {message}
    </div>
  );
}

export function Thumb({ src, className = "" }: { src: string | null; className?: string }) {
  return src ? (
    <img src={src} loading="lazy" referrerPolicy="no-referrer" className={`block aspect-[16/10] w-full bg-edge object-cover ${className}`} />
  ) : (
    <div className={`grid aspect-[16/10] w-full place-items-center bg-edge text-xs text-muted ${className}`}>no preview</div>
  );
}

export function Card({
  children,
  selected,
  dimmed,
  interactive = true,
  onClick,
}: {
  children: ReactNode;
  selected?: boolean;
  dimmed?: boolean;
  interactive?: boolean;
  onClick?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      className={`overflow-hidden rounded-[10px] border-2 bg-card transition ${selected ? "border-pink" : "border-transparent"} ${dimmed ? "opacity-50" : ""} ${interactive ? "cursor-pointer hover:-translate-y-0.5 hover:border-accent" : ""}`}
    >
      {children}
    </div>
  );
}

export const field =
  "rounded-md border border-edge bg-card px-2 py-1 text-sm text-fg outline-none focus:border-accent";
