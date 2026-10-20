// A small inline icon set: decorative only (aria-hidden), so every control
// that uses one must also carry a text label.

const PATHS = {
  folder: "M2.5 4.5h4l1.5 1.5h5.5v7h-11z",
  file: "M4 2.5h5l3 3v8H4z M9 2.5v3h3",
  chevronRight: "M6 4l4 4-4 4",
  chevronDown: "M4 6l4 4 4-4",
  search: "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10z M10.6 10.6l3.4 3.4",
  sparkle: "M8 1.5l1.4 4.1 4.1 1.4-4.1 1.4L8 12.5 6.6 8.4 2.5 7l4.1-1.4z M13 11.5l.6 1.4 1.4.6-1.4.6-.6 1.4-.6-1.4-1.4-.6 1.4-.6z",
  gear: "M8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z M8 1.5v2 M8 12.5v2 M1.5 8h2 M12.5 8h2 M3.4 3.4l1.4 1.4 M11.2 11.2l1.4 1.4 M3.4 12.6l1.4-1.4 M11.2 4.8l1.4-1.4",
  help: "M8 14.5a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13z M6.2 6.2a1.9 1.9 0 1 1 2.6 1.8c-.5.2-.8.6-.8 1.1v.4 M8 11.5v.1",
  close: "M4 4l8 8 M12 4l-8 8",
  warning: "M8 2l6.5 11.5h-13z M8 6.5v3.5 M8 12v.1",
  check: "M3 8.5l3 3 7-7",
  arrowUp: "M8 13V3 M4 7l4-4 4 4",
  eye: "M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z M8 10a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
  eyeOff: "M2 2l12 12 M6.6 6.6a2 2 0 0 0 2.8 2.8 M4.2 4.4C2.6 5.6 1.5 8 1.5 8s2.5 4.5 6.5 4.5c1.3 0 2.4-.4 3.3-1 M7 3.6c.3 0 .7-.1 1-.1 4 0 6.5 4.5 6.5 4.5s-.6 1.1-1.6 2.2",
  folderPlus: "M2.5 4.5h4l1.5 1.5h5.5v7h-11z M8 8v3.5 M6.25 9.75h3.5",
  refresh: "M13 8a5 5 0 1 1-1.5-3.6 M13 2.5v2.5h-2.5",
  terminal: "M2.5 3.5h11v9h-11z M5 6.5l2 1.5-2 1.5 M8.5 10h2.5",
  copy: "M5.5 5.5h7v8h-7z M3.5 10.5v-8h7",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg
      className={`icon ${className ?? ""}`}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
