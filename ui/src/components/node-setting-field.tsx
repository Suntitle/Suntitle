import { Checkbox } from "@/components/ui/checkbox"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import type { NodeSettingDefinition, NodeSettingValue } from "@/stores/models"

function getOptionValue(setting: NodeSettingDefinition, serializedValue: string): NodeSettingValue {
  return setting.options?.find((option) => String(option.value) === serializedValue)?.value ?? serializedValue
}

export function SettingField({ setting, value, onChange }: { setting: NodeSettingDefinition; value: NodeSettingValue | undefined; onChange: (value: NodeSettingValue) => void }) {
  const id = `node-setting-${setting.key}`

  if (setting.input === "checkbox") {
    return (
      <div className="flex items-start gap-3 pt-1 sm:col-span-2">
        <Checkbox id={id} checked={Boolean(value)} onCheckedChange={(checked) => onChange(checked)} />
        <div className="grid gap-1">
          <Label htmlFor={id}>{setting.label}</Label>
          {setting.description && <p className="text-xs text-muted-foreground">{setting.description}</p>}
        </div>
      </div>
    )
  }

  if (setting.input === "select") {
    return (
      <div className="grid gap-2">
        <Label htmlFor={id}>{setting.label}</Label>
        <Select
          id={id}
          required={setting.required}
          items={Object.fromEntries((setting.options ?? []).map((option) => [String(option.value), option.label]))}
          value={value === null || value === undefined ? null : String(value)}
          onValueChange={(nextValue) => nextValue !== null && onChange(getOptionValue(setting, nextValue))}
        >
          <SelectTrigger><SelectValue placeholder={`Select ${setting.label.toLowerCase()}`} /></SelectTrigger>
          <SelectContent>
            {setting.options?.map((option) => <SelectItem key={String(option.value)} value={String(option.value)}>{option.label}</SelectItem>)}
          </SelectContent>
        </Select>
        {setting.description && <p className="text-xs text-muted-foreground">{setting.description}</p>}
      </div>
    )
  }

  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{setting.label}</Label>
      <Input
        id={id}
        type={setting.input === "number" ? "number" : "text"}
        value={value === null || value === undefined ? "" : String(value)}
        min={setting.minimum}
        max={setting.maximum}
        step={setting.valueType === "integer" ? 1 : "any"}
        placeholder={setting.placeholder}
        required={setting.required}
        onChange={(event) => {
          if (setting.input !== "number") return onChange(event.target.value)
          onChange(event.target.value === "" ? null : Number(event.target.value))
        }}
      />
      {setting.description && <p className="text-xs text-muted-foreground">{setting.description}</p>}
    </div>
  )
}
