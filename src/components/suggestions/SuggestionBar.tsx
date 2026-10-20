import { useEffect, useRef } from "react";
import { focusRegion, registerRegion } from "../../focusRegistry";
import { hasPlaceholders, insertCommand, runCommand } from "../../commands";
import { useAppStore } from "../../state/store";
import { CommandTape } from "../common/CommandTape";
import { Icon } from "../common/Icon";
import { RiskBadge } from "../common/RiskBadge";

/** Fixes for the last failed command, shown under the terminal. */
export function SuggestionBar() {
  const suggestions = useAppStore((s) => s.suggestions);
  const setSuggestions = useAppStore((s) => s.setSuggestions);
  const sectionRef = useRef<HTMLElement>(null);

  useEffect(
    () =>
      registerRegion("suggestions", () => {
        const target = sectionRef.current?.querySelector<HTMLElement>(".suggestion button, button");
        target?.focus();
        return Boolean(target);
      }),
    [],
  );

  if (!suggestions) return null;

  const dismiss = () => {
    setSuggestions(null);
    focusRegion("terminal");
  };

  return (
    <section
      ref={sectionRef}
      className="suggestions"
      aria-labelledby="suggestions-heading"
      data-region="suggestions"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          dismiss();
        }
      }}
    >
      <div className="suggestions-header">
        <h2 id="suggestions-heading">Suggested fixes</h2>
        <span className="failed-command" title={suggestions.commandLine}>
          <span className="visually-hidden">for the command </span>
          {suggestions.commandLine}
        </span>
        <button type="button" className="icon-button" aria-label="Dismiss suggested fixes" onClick={dismiss}>
          <Icon name="close" />
        </button>
      </div>
      <ul className="suggestion-list">
        {suggestions.items.map((suggestion) => {
          const { command } = suggestion;
          const incomplete = command ? hasPlaceholders(command) : false;
          return (
            <li key={`${suggestion.rule}:${command ?? suggestion.title}`} className="suggestion">
              <div>
                <h3>{suggestion.title}</h3>
                <p>{suggestion.explanation}</p>
                {command && <CommandTape command={command} label="Suggested command:" />}
              </div>
              {command && (
                <div className="suggestion-actions">
                  {suggestion.risk && <RiskBadge risk={suggestion.risk} />}
                  <button type="button" className="button" onClick={() => void insertCommand(command)}>
                    Put on command line
                  </button>
                  <button
                    type="button"
                    className="button primary"
                    disabled={incomplete}
                    title={incomplete ? "Fill in the placeholders first" : undefined}
                    onClick={() => void runCommand(command, suggestion.risk)}
                  >
                    Run
                  </button>
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
