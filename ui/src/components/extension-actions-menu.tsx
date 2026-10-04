import { Ellipsis, RefreshCw } from "lucide-react"
import { observer } from "mobx-react-lite"

import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useStores } from "@/stores"
import type { Extension } from "@/stores/models"

export const ExtensionActionsMenu = observer(function ExtensionActionsMenu({ extension }: { extension: Extension }) {
  const { extensionsStore } = useStores()
  const isPending = extensionsStore.isRefreshPending(extension.name)

  async function refreshExtension() {
    try {
      await extensionsStore.refreshExtension(extension.name)
    } catch {
      // ExtensionsStore exposes the normalized API error to this observer.
    }
  }

  return (
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
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={refreshExtension}>
          <RefreshCw aria-hidden="true" />
          Refresh extension
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
})
