import { useEffect, useState } from "react"
import { Blocks, Plus } from "lucide-react"
import { observer } from "mobx-react-lite"

import { ExtensionActionsMenu } from "@/components/extension-actions-menu"
import { InstallExtensionDialog } from "@/components/install-extension-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { useStores } from "@/stores"
import type { Extension, ExtensionSourceType } from "@/stores/models"

const sourceTypeLabels: Record<ExtensionSourceType, string> = {
  local: "Local",
  git: "Git",
  oci: "OCI",
  http: "HTTP",
}

/** Label resource publication conditions; registration never implies installation. */
function publicationStatus(extension: Extension): string {
  const status = extension.status
  if (!status) return "Pending"
  const conditions = status.conditions ?? []
  const ready = conditions.find((item) => item.type === "Ready")
  if (ready?.status === "True") return "Published"
  if (ready?.status === "False" && ready.reason === "PreparationFailed") return "Failed"
  if (ready?.status === "False" && ready.reason === "Preparing") return "Preparing"
  return "Pending"
}

/** Render registered extensions and reconcile asynchronous publication state. */
export const ExtensionsPage = observer(function ExtensionsPage() {
  const { extensionsStore } = useStores()
  const [isInstallDialogOpen, setIsInstallDialogOpen] = useState(false)

  useEffect(() => {
    let active = true
    let timer: ReturnType<typeof setTimeout>
    /** Reconcile publication status while the page is mounted. */
    async function reconcile() {
      if (!extensionsStore.isInstalling && extensionsStore.pendingRefreshNames.size === 0) {
        await extensionsStore.loadExtensions().catch(() => undefined)
      }
      if (active) timer = setTimeout(reconcile, 5000)
    }
    void reconcile()
    return () => { active = false; clearTimeout(timer) }
  }, [extensionsStore])

  return (
    <>
      <section className="min-w-0 flex-1 px-5 py-8 sm:px-8 md:px-10 md:py-10 lg:px-12 lg:py-12">
        <div className="mx-auto w-full max-w-6xl">
          <header className="flex flex-col gap-5 sm:flex-row sm:items-end sm:justify-between">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.18em] text-violet-600">Workspace</p>
              <h1 className="mt-2 text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">Extensions</h1>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground sm:text-base">
                Register and manage extensions for your subtitle workspace.
              </p>
            </div>
            <Button className="h-9 self-start sm:self-auto" onClick={() => setIsInstallDialogOpen(true)}>
              <Plus aria-hidden="true" />
              Register extension
            </Button>
          </header>

          <Card className="mt-8 overflow-hidden border-violet-100/80 bg-white shadow-xl shadow-violet-950/[.06]">
            <CardHeader className="flex flex-row items-start justify-between gap-4 border-b border-violet-100/80 bg-gradient-to-r from-white to-violet-50/50 px-5 py-5 sm:px-6">
              <div>
                <CardTitle className="text-base tracking-[-0.015em] sm:text-lg">Registered extensions</CardTitle>
                <CardDescription className="mt-1">Extensions currently registered with Suntitle.</CardDescription>
              </div>
              <Badge variant="outline" className="border-violet-200 bg-violet-50 px-2.5 py-1 text-violet-700">
                {extensionsStore.total} {extensionsStore.total === 1 ? "extension" : "extensions"}
              </Badge>
            </CardHeader>
            <CardContent className="p-0">
              {extensionsStore.error && extensionsStore.items.length > 0 && <p role="alert" className="px-6 py-3 text-sm text-destructive">{extensionsStore.error}</p>}
              <Table>
                <TableHeader className="bg-violet-50/60">
                  <TableRow className="hover:bg-violet-50/60">
                    <TableHead className="pl-5 sm:pl-6">Extension</TableHead>
                    <TableHead>Publication</TableHead>
                    <TableHead>Source type</TableHead>
                    <TableHead>Source</TableHead>
                    <TableHead className="w-16 pr-5 text-right sm:pr-6"><span className="sr-only">Actions</span></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {extensionsStore.isLoading && extensionsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent"><TableCell colSpan={5} className="h-24 text-center text-muted-foreground">Loading extensions…</TableCell></TableRow>
                  )}
                  {extensionsStore.error && extensionsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent"><TableCell colSpan={5} className="h-24 text-center text-destructive">{extensionsStore.error}</TableCell></TableRow>
                  )}
                  {!extensionsStore.isLoading && !extensionsStore.error && extensionsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent"><TableCell colSpan={5} className="h-24 text-center text-muted-foreground">No extensions are registered yet.</TableCell></TableRow>
                  )}
                  {extensionsStore.items.map((extension) => (
                    <TableRow key={extension.name} className="hover:bg-violet-50/35">
                      <TableCell className="pl-5 sm:pl-6">
                        <span className="flex items-center gap-3">
                          <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-violet-100 text-violet-700"><Blocks className="size-4" aria-hidden="true" /></span>
                          <span className="min-w-0">
                            <span className="block font-medium">{extension.displayName}</span>
                            <span className="block max-w-sm truncate text-xs text-muted-foreground">{extension.adapter.type}</span>
                          </span>
                        </span>
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline">{publicationStatus(extension)}</Badge>
                        {extension.status?.conditions?.filter((condition) => condition.type === "Ready").map((condition) => <span key={condition.type} className="mt-1 block max-w-64 whitespace-normal text-xs text-muted-foreground">{condition.message || condition.reason}</span>)}
                        {extension.status?.artifactReference && <span className="mt-1 block max-w-48 truncate font-mono text-xs text-muted-foreground" title={extension.status.artifactReference}>{publicationStatus(extension) === "Pending" ? "Previous artifact: " : ""}{extension.status.artifactReference}</span>}
                      </TableCell>
                      <TableCell><Badge variant="outline">{sourceTypeLabels[extension.sourceType]}</Badge></TableCell>
                      <TableCell className="max-w-72 truncate font-mono text-xs text-muted-foreground" title={extension.source}>{extension.source}</TableCell>
                      <TableCell className="pr-5 text-right sm:pr-6">
                        <div className="flex flex-col items-end gap-1">
                          <ExtensionActionsMenu extension={extension} />
                          {extensionsStore.refreshError(extension.name) && <span role="alert" className="max-w-48 text-right text-xs text-destructive">{extensionsStore.refreshError(extension.name)}</span>}
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </div>
      </section>

      <InstallExtensionDialog open={isInstallDialogOpen} onOpenChange={setIsInstallDialogOpen} />
    </>
  )
})
