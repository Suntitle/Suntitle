import { useState, type FormEvent } from "react"
import { observer } from "mobx-react-lite"

import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { useStores } from "@/stores"
import type { ExtensionSourceType } from "@/stores/models"

/** Controlled source-registration dialog lifecycle. */
interface InstallExtensionDialogProps {
  /** Whether the dialog is visible. */
  open: boolean
  /** Receives requested visibility changes. */
  onOpenChange: (open: boolean) => void
}

const sourceTypeLabels: Record<ExtensionSourceType, string> = { local: "Local", git: "Git", oci: "OCI", http: "HTTP" }

/** Collect source location and separate OCI manifest contents for controller resolution. */
export const InstallExtensionDialog = observer(function InstallExtensionDialog({ open, onOpenChange }: InstallExtensionDialogProps) {
  const { extensionsStore } = useStores()
  const [sourceType, setSourceType] = useState<ExtensionSourceType>("local")
  const [source, setSource] = useState("")
  const [manifest, setManifest] = useState("")

  /** Clear the draft after cancellation or successful registration. */
  function resetForm() {
    setSourceType("local")
    setSource("")
    setManifest("")
    extensionsStore.clearInstallError()
  }

  /** Retain the draft while the API persists registration. */
  function handleOpenChange(nextOpen: boolean) {
    if (extensionsStore.isInstalling) return
    if (!nextOpen) resetForm()
    onOpenChange(nextOpen)
  }

  /** Register the selected source without asking users to duplicate manifest fields. */
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!source.trim() || (sourceType === "oci" && !manifest.trim()) || extensionsStore.isInstalling) return
    try {
      await extensionsStore.installExtension({ sourceType, source: source.trim(), ...(sourceType === "oci" ? { manifest } : {}) })
      resetForm()
      onOpenChange(false)
    } catch {
      // The store retains API errors alongside the submitted location.
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-lg">
        <form onSubmit={handleSubmit}>
          <DialogHeader className="border-b px-6 py-5 pr-14">
            <DialogTitle>Register extension</DialogTitle>
            <DialogDescription>Add an extension from its path or URL.</DialogDescription>
          </DialogHeader>
          <fieldset disabled={extensionsStore.isInstalling} className="grid min-w-0 gap-5 px-6 py-6">
            <div>
              <h2 className="text-sm font-semibold">Extension source</h2>
              <p className="mt-2 text-xs text-muted-foreground">{sourceType === "oci" ? "Provide the binary artifact and its manifest separately." : "Suntitle reads the name, runtime, and settings from its manifest."}</p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="extension-source-type">Source type</Label>
              <Select disabled={extensionsStore.isInstalling} value={sourceType} onValueChange={(value) => { setSourceType(value as ExtensionSourceType); setSource(""); setManifest(""); extensionsStore.clearInstallError() }}>
                <SelectTrigger id="extension-source-type" aria-label="Source type"><SelectValue>{sourceTypeLabels[sourceType]}</SelectValue></SelectTrigger>
                <SelectContent>{Object.entries(sourceTypeLabels).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
              </Select>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="extension-source">{sourceType === "oci" ? "OCI URL" : "Extension path or URL"}</Label>
              <Input id="extension-source" value={source} required pattern=".*\S.*" placeholder={sourceType === "local" ? "/opt/suntitle/extensions/hello" : sourceType === "oci" ? "registry.example.com/extension:latest" : sourceType === "git" ? "https://github.com/example/extension.git" : "https://example.com/hello.py"} onChange={(event) => { setSource(event.target.value); extensionsStore.clearInstallError() }} />
            </div>
            {sourceType === "oci" && (
              <div className="grid gap-2">
                <Label htmlFor="extension-manifest">Manifest (YAML or JSON)</Label>
                <Textarea id="extension-manifest" required maxLength={1048576} rows={7} className="min-h-40 max-h-64" value={manifest} placeholder={"displayName: Hello\nadapter:\n  execution:\n    command: ./hello"} onChange={(event) => { setManifest(event.target.value); extensionsStore.clearInstallError() }} aria-describedby="extension-manifest-help" />
                <p id="extension-manifest-help" className="text-xs text-muted-foreground">The controller parses this manifest; the OCI artifact contains the binary.</p>
              </div>
            )}
            {extensionsStore.installError && <p role="alert" className="text-sm text-destructive">{extensionsStore.installError}</p>}
          </fieldset>
          <DialogFooter className="items-center justify-between border-t bg-muted/30 px-6 py-4">
            <span className="mr-auto text-xs text-muted-foreground">Local · Git · OCI · HTTP</span>
            <Button type="button" variant="outline" disabled={extensionsStore.isInstalling} onClick={() => handleOpenChange(false)}>Cancel</Button>
            <Button type="submit" disabled={!source.trim() || (sourceType === "oci" && !manifest.trim()) || extensionsStore.isInstalling}>{extensionsStore.isInstalling ? "Registering…" : "Register extension"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
})
