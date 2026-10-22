import { AREA_SHORTCUTS, SHORTCUTS } from "../shortcuts";
import { useAppStore } from "../state/store";
import { Dialog } from "./common/Dialog";

export function HelpDialog() {
  const close = () => useAppStore.getState().openOverlay(null);
  return (
    <Dialog title="Keyboard shortcuts" onClose={close} footer={<button type="button" className="button primary" onClick={close}>Close</button>}>
      <p>
        Everything in InTerminal works from the keyboard. The terminal is a normal PowerShell; the file tree, search and AI
        are there to help you learn which command does what.
      </p>
      <p className="muted">
        Many command examples in search come from tldr-pages (tldr.sh) by the tldr-pages contributors, shared under
        the Creative Commons Attribution 4.0 licence.
      </p>
      <table className="shortcut-table">
        <caption className="visually-hidden">Shortcuts that work anywhere</caption>
        <thead>
          <tr>
            <th scope="col">Keys</th>
            <th scope="col">Anywhere in InTerminal</th>
          </tr>
        </thead>
        <tbody>
          {SHORTCUTS.map((s) => (
            <tr key={s.id}>
              <td>
                <kbd>{s.keys}</kbd>
              </td>
              <td>{s.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <table className="shortcut-table">
        <caption className="visually-hidden">Shortcuts in specific areas</caption>
        <thead>
          <tr>
            <th scope="col">Keys</th>
            <th scope="col">Area</th>
            <th scope="col">What it does</th>
          </tr>
        </thead>
        <tbody>
          {AREA_SHORTCUTS.map((s) => (
            <tr key={`${s.area}-${s.keys}`}>
              <td>
                <kbd>{s.keys}</kbd>
              </td>
              <td>{s.area}</td>
              <td>{s.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Dialog>
  );
}
