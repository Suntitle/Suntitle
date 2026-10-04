import { useState, type ChangeEvent, type FormEvent } from "react"
import { FileInput } from "lucide-react"
import { observer } from "mobx-react-lite"

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
import { useStores } from "@/stores"
import type { ExtensionSourceType } from "@/stores/models"

interface InstallExtensionDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const sourceTypeLabels: Record<ExtensionSourceType, string> = {
  local: "Local",
  git: "Git",
  zip: "ZIP",
}

export const InstallExtensionDialog = observer(function InstallExtensionDialog({ open, onOpenChange }: InstallExtensionDialogProps) {
  const { extensionsStore } = useStores()
  const [sourceType, setSourceType] = useState<ExtensionSourceType>("local")
  const [source, setSource] = useState("")

  function resetForm() {
    setSourceType("local")
    setSource("")
    extensionsStore.clearInstallError()
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen && !extensionsStore.isInstalling) resetForm()
    onOpenChange(nextOpen)
  }

  function handleLocalSource(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0]
    setSource(file?.webkitRelativePath || file?.name || "")
    extensionsStore.clearInstallError()
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!source) return

    try {
      await extensionsStore.installExtension({ sourceType, source })
      handleOpenChange(false)
    } catch {
      // ExtensionsStore exposes the normalized API error to this observer.
    }
  }

  const isLocal = sourceType === "local"

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-lg">
        <form onSubmit={handleSubmit}>
          <DialogHeader className="border-b px-6 py-5 pr-14">
            <DialogTitle>Install extension</DialogTitle>
            <DialogDescription>Add an extension from a local file, Git repository, or ZIP URL.</DialogDescription>
          </DialogHeader>

          <div className="grid gap-5 px-6 py-6">
            <div className="grid gap-2">
              <Label htmlFor="extension-source-type">Source type</Label>
              <Select
                value={sourceType}
                onValueChange={(value) => {
                  setSourceType(value as ExtensionSourceType)
                  setSource("")
                  extensionsStore.clearInstallError()
                }}
              >
                <SelectTrigger id="extension-source-type" aria-label="Source type" className="h-10">
                  <SelectValue>{sourceTypeLabels[sourceType]}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="local">Local</SelectItem>
                  <SelectItem value="git">Git</SelectItem>
                  <SelectItem value="zip">ZIP</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="grid gap-2">
              <Label htmlFor="extension-source">{isLocal ? "Extension file" : "Source"}</Label>
              {isLocal ? (
                <div className="relative">
                  <FileInput className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
                  <Input
                    key={sourceType}
                    id="extension-source"
                    type="file"
                    className="h-10 pl-9 file:mr-3"
                    required
                    onChange={handleLocalSource}
                  />
                </div>
              ) : (
                <Input
                  key={sourceType}
                  id="extension-source"
                  className="h-10"
                  type="url"
                  value={source}
                  placeholder={sourceType === "git" ? "https://github.com/example/extension.git" : "https://example.com/extension.zip"}
                  required
                  onChange={(event) => {
                    setSource(event.target.value)
                    extensionsStore.clearInstallError()
                  }}
                />
              )}
              <p className="text-xs text-muted-foreground">
                {isLocal ? "The selected file will be validated." : "Suntitle will fetch and validate the extension from this URL."}
              </p>
            </div>

            {extensionsStore.installError && <p role="alert" className="text-sm text-destructive">{extensionsStore.installError}</p>}
          </div>

          <DialogFooter className="border-t bg-muted/30 px-6 py-4">
            <Button type="button" variant="outline" disabled={extensionsStore.isInstalling} onClick={() => handleOpenChange(false)}>Cancel</Button>
            <Button type="submit" disabled={!source || extensionsStore.isInstalling}>
              {extensionsStore.isInstalling ? "Installing…" : "Install extension"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
})
