import { useEffect, useMemo, useState, type FormEvent } from "react"
import { Mic } from "lucide-react"
import { observer } from "mobx-react-lite"

import { SettingField } from "@/components/node-setting-field"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { nodeTypeIcons } from "@/lib/node-icons"
import { cn } from "@/lib/utils"
import { extensionDefinition, extensionFormValues, extensionSettings } from "@/stores/extension-nodes"
import { useStores } from "@/stores"
import type {
  Extension,
  AudioSourceSettings,
  DisplaySettings,
  Node,
  NodeSettingValue,
  NodeStatus,
  NodeRole,
  NodeUpdate,
  TranscriberSettings,
  TranslatorSettings,
} from "@/stores/models"

const nodeTypeLabels: Record<NodeRole, string> = {
  audio_source: "Audio source",
  transcriber: "Transcriber",
  translator: "Translator",
  display: "Display",
  custom: "Custom",
}

const statusStyles: Record<NodeStatus, string> = {
  connected: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  disconnected: "border-neutral-200 bg-neutral-100 text-neutral-600",
}

function formatUpdatedAt(value: string) {
  const date = new Date(value)

  if (Number.isNaN(date.getTime())) return "Unknown"

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date)
}

function formatNodeId(value: string) {
  return value.length > 13 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value
}

/** Preserve custom extension identity and parse instance values before saving. */
function createNodeUpdate(node: Node, name: string, inputNodeId: string | null, settings: Record<string, NodeSettingValue>, extension?: Extension): NodeUpdate {
  const base = {
    name,
    description: node.description ?? null,
    enabled: node.enabled,
    inputNodeId,
  }

  if (node.spec.extensionRef) {
    if (!extension) throw new Error("The referenced extension is unavailable")
    return { ...base, type: node.type, extensionRef: node.spec.extensionRef.name,
      settings: { ...node.spec.settings.custom, ...extensionSettings(extension, settings) } }
  }

  switch (node.type) {
    case "audio_source": return { ...base, type: node.type, settings: settings as unknown as AudioSourceSettings }
    case "transcriber": return { ...base, type: node.type, settings: settings as unknown as TranscriberSettings }
    case "translator": return { ...base, type: node.type, settings: settings as unknown as TranslatorSettings }
    case "display": return { ...base, type: node.type, settings: settings as unknown as DisplaySettings }
    case "custom": throw new Error("Custom nodes require an extension")
  }
}

/** Edit built-in and manifest-declared custom settings using the existing controls. */
export const NodeSettingsDialog = observer(function NodeSettingsDialog() {
  const { nodesStore, extensionsStore } = useStores()
  const node = nodesStore.selectedNode
  const [nodeName, setNodeName] = useState("")
  const [inputNodeId, setInputNodeId] = useState<string | null>(null)
  const [settings, setSettings] = useState<Record<string, NodeSettingValue>>({})

  const extension = extensionsStore.items.find((item) => item.name === node?.spec.extensionRef?.name)
  const builtInDefinition = node ? nodesStore.nodeTypes.find((item) => item.type === node.type) : undefined
  const definition = node?.spec.extensionRef
    ? extension ? extensionDefinition(extension, builtInDefinition) : null
    : builtInDefinition ?? null

  const allowedInputNodes = useMemo(() => {
    if (!node || !definition) return []
    return nodesStore.items.filter((item) => item.id !== node.id && definition.inputTypes.includes(item.type))
  }, [definition, node, nodesStore.items])

  const selectedInput = inputNodeId
    ? nodesStore.items.find((item) => item.id === inputNodeId) ?? null
    : null

  useEffect(() => {
    if (!node) return

    setNodeName(node.name)
    setInputNodeId(node.inputNodeId)
    setSettings(extension ? extensionFormValues(extension, node.spec.settings.custom ?? {}) : node.type === "custom" ? {} : { ...node.settings })
    nodesStore.clearSaveError()

    if (nodesStore.nodeTypes.length === 0) {
      void nodesStore.loadNodeTypes().catch(() => undefined)
    }
  }, [node, nodesStore, extension])

  function updateSetting(key: string, value: NodeSettingValue) {
    setSettings((current) => ({ ...current, [key]: value }))
    nodesStore.clearSaveError()
  }

  function handleOpenChange(open: boolean) {
    if (!open) nodesStore.selectNode(null)
  }

  /** Save the matching settings variant without changing the extension reference. */
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!node || !definition) return

    try {
      const update = createNodeUpdate(node, nodeName, inputNodeId, settings, extension)
      await nodesStore.updateNode(node.id, update)
      nodesStore.selectNode(null)
    } catch (error) {
      nodesStore.setSaveError(error)
    }
  }

  const showInput = (definition?.inputTypes.length ?? 0) > 0
  const requiresInput = !node?.spec.extensionRef && showInput
  const canSave = Boolean(definition && nodeName.trim() && (!requiresInput || inputNodeId))
  const InputIcon = selectedInput ? nodeTypeIcons[selectedInput.type] : Mic

  return (
    <Dialog open={node !== null} onOpenChange={handleOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] max-w-4xl overflow-hidden p-0 sm:max-h-[calc(100svh-3rem)]">
        {node && (
          <form className="flex max-h-[inherit] flex-col" onSubmit={handleSubmit} onChange={nodesStore.clearSaveError}>
            <DialogHeader className="border-b px-6 py-5 pr-14">
              <DialogTitle>Update {node.name}</DialogTitle>
              <DialogDescription>Change this node&apos;s input and type-specific settings.</DialogDescription>
            </DialogHeader>

            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 py-4">
              <section aria-labelledby="node-details-title" className="rounded-xl border bg-neutral-50 p-4">
                <div className="flex items-center justify-between gap-4">
                  <h3 id="node-details-title" className="text-sm font-semibold">Node details</h3>
                  <Badge variant="outline" className={cn("capitalize", statusStyles[node.status])}>{node.status}</Badge>
                </div>
                <dl className="mt-3 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <div><dt className="text-xs text-muted-foreground">Node type</dt><dd className="mt-1 font-mono text-xs font-medium">{node.extensionName ?? nodeTypeLabels[node.type]}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Host</dt><dd className="mt-1 font-mono text-xs font-medium">{node.host.name}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Node ID</dt><dd className="mt-1 font-mono text-xs font-medium">{formatNodeId(node.id)}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Updated</dt><dd className="mt-1 font-mono text-xs font-medium">{formatUpdatedAt(node.updatedAt)}</dd></div>
                </dl>
              </section>

              {showInput && (
                <section aria-labelledby="current-input-title" className="rounded-xl border">
                  <div className="flex items-center justify-between gap-4 px-4 py-3">
                    <h3 id="current-input-title" className="text-sm font-semibold">Current input</h3>
                    <span className="text-xs font-medium">{selectedInput ? "1 source" : "No source"}</span>
                  </div>
                  <div className="border-t px-4 py-3">
                    {selectedInput ? (
                      <div className="flex items-center gap-3">
                        <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-violet-50 text-violet-700"><InputIcon className="size-4" aria-hidden="true" /></span>
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-sm font-medium">{selectedInput.name}</p>
                          <p className="truncate text-xs text-muted-foreground">{nodeTypeLabels[selectedInput.type]} · {selectedInput.host.name}</p>
                        </div>
                        <Badge variant="outline" className={cn("capitalize", statusStyles[selectedInput.status])}>{selectedInput.status}</Badge>
                      </div>
                    ) : (
                      <p className="text-sm text-muted-foreground">No compatible input is connected.</p>
                    )}
                  </div>
                </section>
              )}

              <section aria-labelledby="node-settings-title" className="space-y-4 px-1 py-1">
                <div>
                  <h3 id="node-settings-title" className="text-sm font-semibold">Node settings</h3>
                  <p className="mt-1 text-xs text-muted-foreground">Choose an allowed input, then update settings for this node.</p>
                </div>

                {showInput && (
                  <div className="grid gap-2">
                    <Label htmlFor="node-input">{extension ? "Input (optional)" : "Input"}</Label>
                    <Select
                      id="node-input"
                      required={requiresInput}
                      items={Object.fromEntries(allowedInputNodes.map((item) => [item.id, `${item.name} · ${item.host.name}`]))}
                      value={inputNodeId}
                      onValueChange={(value) => {
                        setInputNodeId(value)
                        nodesStore.clearSaveError()
                      }}
                    >
                      <SelectTrigger aria-label="Input"><SelectValue placeholder="Select an input node" /></SelectTrigger>
                      <SelectContent>
                        {extension && <SelectItem value="">No input</SelectItem>}
                        {allowedInputNodes.map((item) => <SelectItem key={item.id} value={item.id}>{item.name} · {item.host.name}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  </div>
                )}

                {definition && (
                  <div className="space-y-4">
                    <div className="flex items-end justify-between gap-4 border-b pb-2">
                      <div><h4 className="text-xs font-semibold">{definition.name} settings</h4><p className="mt-1 text-xs text-muted-foreground">{definition.description}</p></div>
                      {requiresInput && <span className="shrink-0 text-xs text-muted-foreground">Accepts: {definition.inputTypes.map((type) => nodeTypeLabels[type]).join(", ")}</span>}
                    </div>

                    <div className="grid gap-4 sm:grid-cols-2">
                      <div className="grid gap-2">
                        <Label htmlFor="node-name">Node name</Label>
                        <Input id="node-name" value={nodeName} onChange={(event) => setNodeName(event.target.value)} maxLength={128} required />
                      </div>

                      {definition.settings.map((setting) => (
                        <SettingField key={setting.key} setting={setting} value={settings[setting.key]} onChange={(value) => updateSetting(setting.key, value)} />
                      ))}
                    </div>
                  </div>
                )}

                {nodesStore.isLoadingNodeTypes && !definition && <p className="text-sm text-muted-foreground">Loading node settings…</p>}
                {nodesStore.nodeTypesError && !definition && <p role="alert" className="text-sm text-destructive">{nodesStore.nodeTypesError}</p>}
                {nodesStore.saveError && <p role="alert" className="text-sm text-destructive">{nodesStore.saveError}</p>}
              </section>
            </div>

            <DialogFooter className="items-center border-t px-6 py-4 sm:justify-between">
              <p className="text-xs text-muted-foreground">Changes will be applied asynchronously on {node.host.name}.</p>
              <div className="flex items-center gap-2">
                <Button type="button" variant="outline" onClick={() => nodesStore.selectNode(null)}>Cancel</Button>
                <Button type="submit" className="bg-violet-600 hover:bg-violet-500" disabled={!canSave || nodesStore.isSaving}>
                  {nodesStore.isSaving ? "Saving…" : "Save changes"}
                </Button>
              </div>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
})
