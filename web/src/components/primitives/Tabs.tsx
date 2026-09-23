import "./Tabs.css";

export interface TabItem {
  id: string;
  label: string;
  disabled?: boolean;
}

export interface TabsProps {
  tabs: TabItem[];
  value: string;
  onChange: (id: string) => void;
}

export function Tabs({ tabs, value, onChange }: TabsProps) {
  return (
    <div role="tablist" aria-label="Tabs" className="themis-tabs">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          aria-selected={tab.id === value}
          disabled={tab.disabled}
          className={[
            "themis-tab",
            tab.id === value ? "themis-tab--active" : "",
          ]
            .filter(Boolean)
            .join(" ")}
          onClick={() => {
            if (!tab.disabled) onChange(tab.id);
          }}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
