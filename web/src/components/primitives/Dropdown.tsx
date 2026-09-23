import { useEffect, useRef, useState } from "react";
import "./Dropdown.css";

export interface DropdownItem {
  id: string;
  label: string;
  disabled?: boolean;
}

export interface DropdownProps {
  items: DropdownItem[];
  value?: string | null;
  onSelect: (id: string) => void;
  label?: string;
  placeholder?: string;
  disabled?: boolean;
}

export function Dropdown({
  items,
  value,
  onSelect,
  label,
  placeholder = "Select…",
  disabled = false,
}: DropdownProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const rootRef = useRef<HTMLDivElement>(null);

  const selected = items.find((item) => item.id === value) ?? null;

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: MouseEvent) {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onPointerDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
    };
  }, [open ]);

  function enabledIndices(): number[] {
    return items
      .map((item, index) => ({ item, index }))
      .filter(({ item }) => !item.disabled)
      .map(({ index }) => index);
  }

  function onButtonKeyDown(event: React.KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const enabled = enabledIndices();
      setActiveIndex(enabled[0] ?? -1);
      setOpen(true);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      const enabled = enabledIndices();
      setActiveIndex(enabled[enabled.length - 1] ?? -1);
      setOpen(true);
    }
  }

  function onMenuKeyDown(event: React.KeyboardEvent) {
    const enabled = enabledIndices();
    if (enabled.length === 0) return;
    const pos = enabled.indexOf(activeIndex);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex(enabled[(pos + 1) % enabled.length] ?? enabled[0] ?? -1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex(
        enabled[(pos - 1 + enabled.length) % enabled.length] ??
          enabled[0] ??
          -1,
      );
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (activeIndex >= 0) {
        onSelect(items[activeIndex]?.id ?? "");
        setOpen(false);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
    }
  }

  return (
    <div className="themis-dropdown" ref={rootRef}>
      {label !== undefined && label !== "" && (
        <span className="themis-dropdown-label">{label}</span>
      )}
      <button
        type="button"
        className="themis-dropdown-button"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => {
          const enabled = enabledIndices();
          setActiveIndex(enabled[0] ?? -1);
          setOpen((prev) => !prev);
        }}
        onKeyDown={onButtonKeyDown}
      >
        <span>{selected?.label ?? placeholder}</span>
        <span aria-hidden="true" className="themis-dropdown-caret">
          ▾
        </span>
      </button>
      {open && !disabled && (
        <ul
          role="listbox"
          tabIndex={-1}
          className="themis-dropdown-menu"
          onKeyDown={onMenuKeyDown}
        >
          {items.map((item, index) => (
            <li
              key={item.id}
              role="option"
              aria-selected={item.id === value}
              aria-disabled={item.disabled}
              className={[
                "themis-dropdown-item",
                index === activeIndex ? "themis-dropdown-item--active" : "",
                item.disabled ? "themis-dropdown-item--disabled" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              onMouseEnter={() => {
                if (!item.disabled) setActiveIndex(index);
              }}
              onMouseDown={(event) => {
                event.preventDefault();
                if (!item.disabled) {
                  onSelect(item.id);
                  setOpen(false);
                }
              }}
            >
              {item.label}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
