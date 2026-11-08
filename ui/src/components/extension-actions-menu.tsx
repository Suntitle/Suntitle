import { useState } from "react"
import { Ellipsis, RefreshCw } from "lucide-react"
import { observer } from "mobx-react-lite"

import { ConfirmationDialog } from "@/components/confirmation-dialog"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useStores } from "@/stores"
import type { Extension } from "@/stores/models"

/** Refresh or remove a registered extension using its stable URL identity. */
export const ExtensionActionsMenu = observer(function ExtensionActionsMenu({ extension }: { extension: Extension }) {
  const { extensionsStore } = useStores()
  const [confirmingRemoval, setConfirmingRemoval] = useState(false)
  const isPending = extensionsStore.isRefreshPending(extension.name) || extensionsStore.isRemoving(extension.name)

  /** Persist a refresh request; the page subsequently reconciles publication. */
  async function refreshExtension() {
    try {
      await extensionsStore.refreshExtension(extension.name)
    } catch {
      // ExtensionsStore exposes the normalized API error to this observer.
    }
  }

  /** Open confirmation with a fresh error state. */
  function confirmRemoval() {
    extensionsStore.clearRemovalError(extension.name)
    setConfirmingRemoval(true)
  }

  /** Request removal only after confirmation; failures remain available for retry. */
  async function removeExtension() {
    try {
      await extensionsStore.removeExtension(extension.name)
      setConfirmingRemoval(false)
    } catch {
      // The confirmation dialog displays the store's API error.
    }
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label={`Actions for ${extension.name}`}
              disabled={isPending}
            />
          }
        >
          {isPending ? <RefreshCw className="animate-spin" aria-hidden="true" /> : <Ellipsis aria-hidden="true" />}
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-56">
          <DropdownMenuItem onClick={refreshExtension}>
            <RefreshCw aria-hidden="true" />
            Refresh extension
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onClick={confirmRemoval}>
            Remove extension
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <ConfirmationDialog
        open={confirmingRemoval}
        title={`Remove ${extension.displayName}?`}
        description="This removes the extension and all nodes that use it. Processing through those nodes will stop. This cannot be undone."
        confirmLabel="Remove extension"
        pendingLabel="Removing…"
        destructive
        isPending={extensionsStore.isRemovalPending(extension.name)}
        error={extensionsStore.removalError(extension.name)}
        onConfirm={removeExtension}
        onOpenChange={(open) => {
          if (!open) extensionsStore.clearRemovalError(extension.name)
          setConfirmingRemoval(open)
        }}
      />
    </>
  )
})
