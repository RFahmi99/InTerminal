import { useEffect, useId, useRef, useState } from "react";
import { announce } from "../../a11y/announcer";
import { combineRisk, copyCommand, hasPlaceholders, insertCommand, runCommand } from "../../commands";
import { api, toAppError } from "../../ipc/api";
import type { FilledCommand, SearchHit } from "../../ipc/types";
import { useAppStore } from "../../state/store";
import { CommandTape } from "../common/CommandTape";
import { Dialog } from "../common/Dialog";
import { Icon } from "../common/Icon";
import { RiskBadge } from "../common/RiskBadge";
import { notifyFailure } from "../../notify";

const EXAMPLES = ["unzip a file", "free disk space", "kill port 3000", "find large files", "undo my last git commit"];
const KIND_LABELS = {
  recipe: "",
  cmdlet: "PowerShell command",
  program: "Installed program",
  example: "Example from tldr-pages",
} as const;

/** Natural-language command search: type what you want, pick a command, fill in its values. */
export function CommandPalette() {
  const close = () => useAppStore.getState().openOverlay(null);
  const indexStatus = useAppStore((s) => s.indexStatus);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [active, setActive] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<SearchHit | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listId = useId();

  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      setHits([]);
      setError(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .searchCommands(trimmed, 10)
        .then((results) => {
          if (cancelled) return;
          setHits(results);
          setActive(0);
          setError(null);
          const count = results.length === 1 ? "1 command" : `${results.length} commands`;
          announce(results.length ? `${count} found.` : "No matching commands.", "polite", "search");
        })
        .catch((e) => {
          if (!cancelled) setError(toAppError(e).message);
        });
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query]);

  if (selected) {
    return (
      <CommandDetail
        hit={selected}
        onBack={() => {
          setSelected(null);
          requestAnimationFrame(() => inputRef.current?.focus());
        }}
        onClose={close}
      />
    );
  }

  const choose = (hit: SearchHit | undefined) => {
    if (hit) setSelected(hit);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") setActive((i) => Math.min(hits.length - 1, i + 1));
    else if (event.key === "ArrowUp") setActive((i) => Math.max(0, i - 1));
    else if (event.key === "Enter") choose(hits[active]);
    else return;
    event.preventDefault();
  };

  const activeId = hits[active] ? `${listId}-${active}` : undefined;

  return (
    <Dialog
      title="Find a command"
      onClose={close}
      initialFocus={inputRef}
      header={
        <div className="search-input-row">
          <Icon name="search" />
          <input
            ref={inputRef}
            role="combobox"
            aria-label="Describe what you want to do"
            aria-expanded={hits.length > 0}
            aria-controls={listId}
            aria-activedescendant={activeId}
            aria-autocomplete="list"
            placeholder="Describe what you want to do, like “extract a zip file”"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
            spellCheck={false}
          />
          <kbd>Esc</kbd>
        </div>
      }
    >
      {indexStatus?.state === "building" && <p className="muted">Command search is still learning what's installed. Results may be incomplete.</p>}
      {indexStatus?.messages.map((message) => (
        <p key={message} className="muted">
          {message}
        </p>
      ))}
      {error && <p className="notice error">{error}</p>}
      {!query.trim() && (
        <div className="search-hints">
          Describe a task in your own words. Results only include commands available on this computer.
          <ul aria-label="Example searches">
            {EXAMPLES.map((example) => (
              <li key={example}>
                <button type="button" className="chip" onClick={() => setQuery(example)}>
                  {example}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {query.trim() && !error && hits.length === 0 && (
        <p className="muted">No matching commands. Try different words, or use “Write a command with AI”.</p>
      )}
      <ul id={listId} className="results" role="listbox" aria-label="Matching commands" hidden={hits.length === 0}>
        {hits.map((hit, i) => (
          <li
            key={hit.id}
            id={`${listId}-${i}`}
            role="option"
            aria-selected={i === active}
            className="result"
            onMouseMove={() => setActive(i)}
            onClick={() => choose(hit)}
          >
            <span className="result-title">{hit.title}</span>
            <span>
              {KIND_LABELS[hit.kind] && <span className="muted">{KIND_LABELS[hit.kind]} </span>}
              {hit.risk.level !== "safe" && <RiskBadge risk={hit.risk} />}
            </span>
            <span className="result-command">
              <CommandTape command={hit.command} />
            </span>
            <span className="result-description">{hit.description}</span>
            {hit.usage && (
              <span className="result-description usage">
                <span className="visually-hidden">Usage: </span>
                {hit.usage}
              </span>
            )}
          </li>
        ))}
      </ul>
    </Dialog>
  );
}

function CommandDetail({ hit, onBack, onClose }: { hit: SearchHit; onBack: () => void; onClose: () => void }) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [filled, setFilled] = useState<FilledCommand | null>(null);
  const firstFieldRef = useRef<HTMLInputElement>(null);
  const insertRef = useRef<HTMLButtonElement>(null);

  // Live preview only; replies to superseded values are ignored.
  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      api
        .fillTemplate(hit.command, values)
        .then((result) => {
          if (!cancelled) setFilled(result);
        })
        .catch((e) => {
          if (!cancelled) notifyFailure(e, "Couldn't build the command");
        });
    }, 80);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [hit.command, values]);

  const command = filled?.command ?? hit.command;
  const risk = filled ? combineRisk(hit.risk, filled.risk) : hit.risk;
  const incomplete = hasPlaceholders(command);

  /**
   * Builds the command from the values as they are right now: the preview
   * can lag a fast typist, and acting on it could insert or run a command
   * with an older value.
   */
  const current = async () => {
    try {
      return await api.fillTemplate(hit.command, values);
    } catch (e) {
      notifyFailure(e, "Couldn't build the command");
      return null;
    }
  };

  const insert = async () => {
    const latest = await current();
    if (latest && (await insertCommand(latest.command))) onClose();
  };
  const run = async () => {
    const latest = await current();
    if (!latest || latest.missing.length > 0) return;
    onClose();
    void runCommand(latest.command, combineRisk(hit.risk, latest.risk));
  };

  return (
    <Dialog
      title={hit.title}
      onClose={onBack}
      initialFocus={hit.params.length ? firstFieldRef : insertRef}
      footer={
        <>
          <button type="button" className="button quiet" onClick={onBack}>
            Back to results
          </button>
          <button type="button" className="button" onClick={() => void copyCommand(command)}>
            <Icon name="copy" /> Copy
          </button>
          <button ref={insertRef} type="submit" form="command-detail-form" className="button">
            Put on command line
          </button>
          <button
            type="button"
            className={`button ${risk.level === "danger" ? "danger" : "primary"}`}
            disabled={incomplete}
            title={incomplete ? "Fill in every value first" : "Ctrl+Enter"}
            aria-keyshortcuts="Control+Enter"
            onClick={() => void run()}
          >
            Run
          </button>
        </>
      }
    >
      <p>{hit.description}</p>
      {hit.usage && (
        <div>
          <p className="muted">Parameters it takes:</p>
          <code className="usage">{hit.usage}</code>
        </div>
      )}
      {hit.kind === "example" && (
        <p className="muted">Example from tldr-pages (tldr.sh), shared under the CC BY 4.0 licence.</p>
      )}
      <CommandTape command={command} block />
      <RiskBadge risk={risk} showReasons />
      <form
        id="command-detail-form"
        onSubmit={(event) => {
          event.preventDefault();
          void insert();
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && event.ctrlKey) {
            event.preventDefault();
            void run();
          }
        }}
        style={{ display: "grid", gap: "var(--space-3)" }}
      >
        {hit.params.map((param, i) => (
          <div className="field" key={param.name}>
            <label htmlFor={`param-${param.name}`}>{param.name}</label>
            <input
              ref={i === 0 ? firstFieldRef : undefined}
              id={`param-${param.name}`}
              className="text-input"
              value={values[param.name] ?? ""}
              aria-describedby={`param-${param.name}-hint`}
              onChange={(event) => setValues((current) => ({ ...current, [param.name]: event.target.value }))}
              spellCheck={false}
            />
            <span id={`param-${param.name}-hint`} className="hint">
              {param.description}
            </span>
          </div>
        ))}
      </form>
      {hit.example && (
        <div>
          <p className="muted">Example:</p>
          <CommandTape command={hit.example} label="Example command:" />
        </div>
      )}
      <p className="muted">
        Enter puts the command on the command line so you can check it. Ctrl+Enter runs it straight away.
      </p>
    </Dialog>
  );
}
