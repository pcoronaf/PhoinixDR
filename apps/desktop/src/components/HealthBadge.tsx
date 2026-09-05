import type { HealthCategory } from "../types";
import { CATEGORY_LABEL } from "../lib/filters";

const CLASS: Record<HealthCategory, string> = {
  Excellent: "h-excellent",
  VeryGood: "h-verygood",
  Good: "h-good",
  Poor: "h-poor",
  VeryPoor: "h-verypoor",
  Unrecoverable: "h-unrecoverable",
  Unknown: "h-unknown",
};

export function HealthBadge({ category, likelihood, confidence }: { category: HealthCategory; likelihood: number; confidence?: number }) {
  return (
    <span className={`health ${CLASS[category]}`} title={confidence === undefined ? undefined : `Assessment confidence ${confidence}%`}>
      <span className="dot" />
      {CATEGORY_LABEL[category]} <span className="pct">{likelihood}%</span>
    </span>
  );
}
