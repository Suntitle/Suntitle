import { useState, type FormEvent } from "react"
import { observer } from "mobx-react-lite"

import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
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

/** Collect source location only; the server reads all configuration from its manifest. */
export const InstallExtensionDialog = observer(function InstallExtensionDialog({ open, onOpenChange }: InstallExtensionDialogProps) {
  const { extensionsStore } = useStores()
  const [sourceType, setSourceType] = useState<ExtensionSourceType>("local")
  const [source, setSource] = useState("")

  /** Clear the draft after cancellation or successful registration. */
  function resetForm() {
    setSourceType("local")
    setSource("")
    extensionsStore.clearInstallError()
  }

  /** Retain the draft while the server reads the manifest and registers it. */
  function handleOpenChange(nextOpen: boolean) {
    if (extensionsStore.isInstalling) return
    if (!nextOpen) resetForm()
    onOpenChange(nextOpen)
  }

  /** Register the selected source without asking users to duplicate manifest fields. */
  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!source.trim() || extensionsStore.isInstalling) return
    try {
      await extensionsStore.installExtension({ sourceType, source: source.trim() })
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
              <p className="mt-2 text-xs text-muted-foreground">Suntitle reads the name, runtime, and settings from its manifest.</p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="extension-source-type">Source type</Label>
              <Select disabled={extensionsStore.isInstalling} value={sourceType} onValueChange={(value) => { setSourceType(value as ExtensionSourceType); setSource(""); extensionsStore.clearInstallError() }}>
                <SelectTrigger id="extension-source-type" aria-label="Source type"><SelectValue>{sourceTypeLabels[sourceType]}</SelectValue></SelectTrigger>
                <SelectContent>{Object.entries(sourceTypeLabels).map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}</SelectContent>
              </Select>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="extension-source">Extension path or URL</Label>
              <Input id="extension-source" value={source} required pattern=".*\S.*" placeholder={sourceType === "local" ? "/opt/suntitle/extensions/hello" : sourceType === "oci" ? "registry.example.com/extension:latest" : sourceType === "git" ? "https://github.com/example/extension.git" : "https://example.com/hello.py"} onChange={(event) => { setSource(event.target.value); extensionsStore.clearInstallError() }} />
            </div>
            {extensionsStore.installError && <p role="alert" className="text-sm text-destructive">{extensionsStore.installError}</p>}
          </fieldset>
          <DialogFooter className="items-center justify-between border-t bg-muted/30 px-6 py-4">
            <span className="mr-auto text-xs text-muted-foreground">Local · Git · OCI · HTTP</span>
            <Button type="button" variant="outline" disabled={extensionsStore.isInstalling} onClick={() => handleOpenChange(false)}>Cancel</Button>
            <Button type="submit" disabled={!source.trim() || extensionsStore.isInstalling}>{extensionsStore.isInstalling ? "Registering…" : "Register extension"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
})
