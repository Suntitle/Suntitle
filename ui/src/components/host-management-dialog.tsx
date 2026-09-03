import { useCallback, useEffect, useState, type FormEvent } from "react"
import { Boxes } from "lucide-react"
import { observer } from "mobx-react-lite"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { NodeActionsMenu } from "@/components/node-actions-menu"
import { SettingField } from "@/components/node-setting-field"
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
import { cn } from "@/lib/utils"
import { useStores } from "@/stores"
import type {
  Host,
  NodeCreate,
  NodeSettingValue,
  NodeStatus,
  NodeType,
  NodeTypeDefinition,
} from "@/stores/models"

const nodeStatusStyles: Record<NodeStatus | "disabled", string> = {
  connected: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  disconnected: "border-neutral-200 bg-neutral-100 text-neutral-600",
  disabled: "border-neutral-200 bg-neutral-100 text-neutral-600",
}

function formatLastSeen(value: string) {
  const date = new Date(value)

  if (Number.isNaN(date.getTime())) return "Unknown"

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date)
}

function getDefaultSettings(definition: NodeTypeDefinition): Record<string, NodeSettingValue> {
  return Object.fromEntries(definition.settings.map((setting) => [setting.key, setting.defaultValue ?? null]))
}

function getDefaultNodeName(type: NodeType): string {
  return `live-${type.replaceAll("_", "-")}`
}

interface HostManagementDialogProps {
  host: Host | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export const HostManagementDialog = observer(function HostManagementDialog({ host, open, onOpenChange }: HostManagementDialogProps) {
  const { nodesStore } = useStores()
  const [selectedType, setSelectedType] = useState<NodeType | null>(null)
  const [nodeName, setNodeName] = useState("")
  const [settings, setSettings] = useState<Record<string, NodeSettingValue>>({})
  const [successMessage, setSuccessMessage] = useState<string | null>(null)

  const selectedDefinition = nodesStore.nodeTypes.find((definition) => definition.type === selectedType) ?? null

  const nodes = host ? nodesStore.nodesForHost(host.id) : []

  const selectDefinition = useCallback((definition: NodeTypeDefinition) => {
    setSelectedType(definition.type)
    setNodeName(getDefaultNodeName(definition.type))
    setSettings(getDefaultSettings(definition))
    setSuccessMessage(null)
    nodesStore.clearInstallError()
  }, [nodesStore])

  useEffect(() => {
    if (!open || !host) return

    setSelectedType(null)
    setNodeName("")
    setSettings({})
    setSuccessMessage(null)
    nodesStore.clearInstallError()

    const requests = [nodesStore.loadNodes()]
    if (nodesStore.nodeTypes.length === 0) requests.push(nodesStore.loadNodeTypes())
    void Promise.all(requests).catch(() => undefined)
  }, [host, nodesStore, open])

  useEffect(() => {
    if (!open || selectedType !== null || nodesStore.nodeTypes.length === 0) return

    const definition = nodesStore.nodeTypes.find((item) => item.type === "transcriber") ?? nodesStore.nodeTypes[0]
    selectDefinition(definition)
  }, [nodesStore.nodeTypes, nodesStore.nodeTypes.length, open, selectedType, selectDefinition])

  function updateSetting(key: string, value: NodeSettingValue) {
    setSettings((current) => ({ ...current, [key]: value }))
    setSuccessMessage(null)
    nodesStore.clearInstallError()
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!host || !selectedDefinition) return

    const create: NodeCreate = {
      type: selectedDefinition.type,
      name: nodeName,
      enabled: true,
      settings,
    }

    try {
      const node = await nodesStore.installNode(host.id, create)
      setSuccessMessage(`${node.name} was installed on ${host.name}.`)
    } catch {
      // NodesStore exposes the normalized API error to this observer.
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] max-w-4xl overflow-hidden p-0 sm:max-h-[calc(100svh-3rem)]">
        {host && (
          <form className="flex max-h-[inherit] flex-col" onSubmit={handleSubmit} onChange={nodesStore.clearInstallError}>
            <DialogHeader className="border-b px-6 py-5 pr-14">
              <DialogTitle>Manage {host.name}</DialogTitle>
              <DialogDescription>Review this host and install another pipeline node.</DialogDescription>
            </DialogHeader>

            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 py-4">
              <section aria-labelledby="host-details-title" className="rounded-xl border bg-neutral-50 p-4">
                <div className="flex items-center justify-between gap-4">
                  <h3 id="host-details-title" className="text-sm font-semibold">Host details</h3>
                  <Badge variant="outline" className="border-emerald-200 bg-emerald-50 capitalize text-emerald-700">{host.status}</Badge>
                </div>
                <dl className="mt-3 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <div><dt className="text-xs text-muted-foreground">IP address</dt><dd className="mt-1 font-mono text-xs font-medium">{host.ip}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Version</dt><dd className="mt-1 font-mono text-xs font-medium">{host.version}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Runtime</dt><dd className="mt-1 font-mono text-xs font-medium">{String(host.metadata?.runtime ?? "Not reported")}</dd></div>
                  <div><dt className="text-xs text-muted-foreground">Last seen</dt><dd className="mt-1 text-xs font-medium">{formatLastSeen(host.lastSeenAt)}</dd></div>
                </dl>
              </section>

              <section aria-labelledby="installed-nodes-title" className="rounded-xl border">
                <div className="flex items-center justify-between gap-4 px-4 py-3">
                  <h3 id="installed-nodes-title" className="text-sm font-semibold">Installed nodes</h3>
                  <span className="text-xs font-medium">{nodes.length} {nodes.length === 1 ? "node" : "nodes"}</span>
                </div>
                <div className="border-t">
                  {nodesStore.isLoading && nodes.length === 0 && <p className="px-4 py-5 text-sm text-muted-foreground">Loading installed nodes…</p>}
                  {nodesStore.error && nodes.length === 0 && <p role="alert" className="px-4 py-5 text-sm text-destructive">{nodesStore.error}</p>}
                  {!nodesStore.isLoading && !nodesStore.error && nodes.length === 0 && <p className="px-4 py-5 text-sm text-muted-foreground">No nodes are installed on this host.</p>}
                  {nodes.map((node) => (
                    <div key={node.id} className="flex items-center gap-3 px-4 py-3">
                      <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-violet-100 text-violet-700"><Boxes className="size-4" aria-hidden="true" /></span>
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-sm font-medium">{node.name}</p>
                        <p className="truncate text-xs text-muted-foreground">{nodesStore.nodeTypes.find((item) => item.type === node.type)?.name ?? node.type.replaceAll("_", " ")} · {host.name}</p>
                      </div>
                      <Badge variant="outline" className={cn("capitalize", nodeStatusStyles[node.enabled ? node.status : "disabled"])}>
                        {node.enabled ? node.status : "disabled"}
                      </Badge>
                      <NodeActionsMenu node={node} />
                    </div>
                  ))}
                </div>
              </section>

              <section aria-labelledby="install-node-title" className="space-y-4 px-1 py-1">
                <div>
                  <h3 id="install-node-title" className="text-sm font-semibold">Install a node</h3>
                  <p className="mt-1 text-xs text-muted-foreground">Choose a node type, then configure the settings required for that node.</p>
                </div>

                <div className="grid gap-2">
                  <Label htmlFor="node-type">Node type</Label>
                  {nodesStore.isLoadingNodeTypes && nodesStore.nodeTypes.length === 0 ? (
                    <p className="text-sm text-muted-foreground">Loading node types…</p>
                  ) : nodesStore.nodeTypesError && nodesStore.nodeTypes.length === 0 ? (
                    <p role="alert" className="text-sm text-destructive">{nodesStore.nodeTypesError}</p>
                  ) : (
                    <Select
                      id="node-type"
                      items={Object.fromEntries(nodesStore.nodeTypes.map((definition) => [definition.type, definition.name]))}
                      value={selectedType}
                      onValueChange={(value) => {
                        const definition = nodesStore.nodeTypes.find((item) => item.type === value)
                        if (definition) selectDefinition(definition)
                      }}
                    >
                      <SelectTrigger aria-label="Node type"><SelectValue placeholder="Select a node type" /></SelectTrigger>
                      <SelectContent>
                        {nodesStore.nodeTypes.map((definition) => <SelectItem key={definition.type} value={definition.type}>{definition.name}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  )}
                </div>

                {selectedDefinition && (
                  <div className="space-y-4">
                    <div className="flex items-end justify-between gap-4 border-b pb-2">
                      <div><h4 className="text-xs font-semibold">{selectedDefinition.name} settings</h4><p className="mt-1 text-xs text-muted-foreground">{selectedDefinition.description}</p></div>
                      <span className="shrink-0 text-xs text-muted-foreground">Type-specific fields</span>
                    </div>

                    <div className="grid gap-4 sm:grid-cols-2">
                      <div className="grid gap-2">
                        <Label htmlFor="node-name">Node name</Label>
                        <Input id="node-name" value={nodeName} onChange={(event) => setNodeName(event.target.value)} maxLength={128} required />
                      </div>

                      {selectedDefinition.settings.map((setting) => (
                        <SettingField key={setting.key} setting={setting} value={settings[setting.key]} onChange={(value) => updateSetting(setting.key, value)} />
                      ))}
                    </div>
                  </div>
                )}

                {nodesStore.installError && <p role="alert" className="text-sm text-destructive">{nodesStore.installError}</p>}
                {successMessage && <p role="status" className="text-sm text-emerald-700">{successMessage}</p>}
              </section>
            </div>

            <DialogFooter className="items-center border-t px-6 py-4 sm:justify-between">
              <p className="text-xs text-muted-foreground">Installation starts immediately on {host.name}.</p>
              <div className="flex items-center gap-2">
                <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
                <Button type="submit" className="bg-violet-600 hover:bg-violet-500" disabled={!selectedDefinition || nodesStore.isInstalling || host.status === "offline"}>
                  {nodesStore.isInstalling ? "Installing…" : "Install node"}
                </Button>
              </div>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
})
