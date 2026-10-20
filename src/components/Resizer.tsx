import { useAppStore } from "../state/store";

const MIN_WIDTH = 180;
const MAX_WIDTH = 640;
const KEY_STEP = 16;

const clamp = (width: number) => Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width)));

/** Drag or use the arrow keys to resize the file tree. */
export function Resizer() {
  const width = useAppStore((s) => s.ui.explorerWidth);
  const setUi = useAppStore((s) => s.setUi);

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const startX = event.clientX;
    const startWidth = width;
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    const onMove = (move: PointerEvent) => setUi({ explorerWidth: clamp(startWidth + move.clientX - startX) });
    const onUp = () => {
      target.removeEventListener("pointermove", onMove);
      target.removeEventListener("pointerup", onUp);
    };
    target.addEventListener("pointermove", onMove);
    target.addEventListener("pointerup", onUp);
  };

  return (
    <div
      className="resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the file tree"
      aria-valuemin={MIN_WIDTH}
      aria-valuemax={MAX_WIDTH}
      aria-valuenow={width}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft") setUi({ explorerWidth: clamp(width - KEY_STEP) });
        else if (event.key === "ArrowRight") setUi({ explorerWidth: clamp(width + KEY_STEP) });
        else return;
        event.preventDefault();
      }}
    />
  );
}
