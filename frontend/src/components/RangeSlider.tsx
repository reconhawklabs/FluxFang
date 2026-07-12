// A discrete snap slider over a fixed list of stops. `value` is the stop
// INDEX (not the underlying quantity), so the parent owns the stop->quantity
// mapping. Used by the Co-Travel page for the distance and time gates.
export interface SliderStop {
  label: string;
}

export interface RangeSliderProps {
  label: string;
  stops: ReadonlyArray<SliderStop>;
  value: number;
  onChange: (index: number) => void;
}

export default function RangeSlider({ label, stops, value, onChange }: RangeSliderProps) {
  const current = stops[value];
  return (
    <label className="flex flex-col gap-1 text-sm text-slate-300">
      <span className="flex items-center justify-between">
        <span>{label}</span>
        <span className="font-mono text-amber-400">{current?.label ?? ''}</span>
      </span>
      <input
        type="range"
        min={0}
        max={stops.length - 1}
        step={1}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="w-full accent-amber-500"
      />
    </label>
  );
}
