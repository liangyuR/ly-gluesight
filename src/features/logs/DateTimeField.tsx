import { DatePicker, Stack, TimePicker } from "qing-steel";

const join = (date: string, time: string) => (date || time ? `${date}T${time}` : "");

export default function DateTimeField({ label, value, disabled, onChange }: {
  label: string; value: string; disabled?: boolean; onChange: (value: string) => void;
}) {
  const [date = "", time = ""] = value.split("T");
  return <Stack gap={6}>
    <DatePicker aria-label={`${label}日期`} value={date} disabled={disabled} onValueChange={next => onChange(join(next, time))} />
    <TimePicker aria-label={`${label}时间`} step={1} value={time} disabled={disabled} onValueChange={next => onChange(join(date, next))} />
  </Stack>;
}
