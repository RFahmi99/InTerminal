// Shows a command the user could type, on the marigold "tape". Every place
// the app proposes a command uses this, so the look itself teaches
// "this is something you can run in the terminal".

const PLACEHOLDER = /(<[a-z0-9-]+>)/;

export function CommandTape({ command, block = false, label }: { command: string; block?: boolean; label?: string }) {
  const parts = command.split(PLACEHOLDER);
  return (
    <code className={`tape${block ? " block" : ""}`}>
      <span className="visually-hidden">{label ?? "Command:"} </span>
      {parts.map((part, i) =>
        PLACEHOLDER.test(part) ? (
          <span key={i} className="placeholder" title="Replace this with a value">
            {part}
          </span>
        ) : (
          part
        ),
      )}
    </code>
  );
}
