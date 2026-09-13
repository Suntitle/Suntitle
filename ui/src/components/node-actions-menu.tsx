import { useState } from "react"
import { Ellipsis } from "lucide-react"
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
import type { Node } from "@/stores/models"

type NodeAction = "restart" | "disable" | "delete"

const actionDetails = {
  restart: {
    verb: "Restart",
    description: "Subtitle processing through this node may pause briefly while it restarts.",
    pendingLabel: "Restarting…",
  },
  disable: {
    verb: "Disable",
    description: "This node will stop processing data and may interrupt downstream output.",
    pendingLabel: "Disabling…",
  },
  delete: {
    verb: "Delete",
    description: "This will permanently remove this node and its settings. Downstream processing may be interrupted. This cannot be undone.",
    pendingLabel: "Deleting…",
  },
} satisfies Record<NodeAction, { verb: string; description: string; pendingLabel: string }>

export const NodeActionsMenu = observer(function NodeActionsMenu({ node }: { node: Node }) {
  const { nodesStore } = useStores()
  const [action, setAction] = useState<NodeAction | null>(null)
  const [isConfirmationOpen, setIsConfirmationOpen] = useState(false)
  const isPending = nodesStore.isNodeActionPending(node.id)
  const error = nodesStore.nodeActionError(node.id)

  function selectAction(nextAction: NodeAction) {
    nodesStore.clearNodeActionError(node.id)
    setAction(nextAction)
    setIsConfirmationOpen(true)
  }

  async function confirmAction() {
    if (!action) return

    try {
      if (action === "restart") await nodesStore.restartNode(node.id)
      else if (action === "delete") await nodesStore.deleteNode(node.id)
      else await nodesStore.disableNode(node)
      setIsConfirmationOpen(false)
    } catch {
      // NodesStore exposes the normalized API error to this observer.
    }
  }

  const details = actionDetails[action ?? "restart"]

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label={`Actions for ${node.name}`}
              disabled={isPending}
            />
          }
        >
          <Ellipsis aria-hidden="true" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={() => selectAction("restart")}>Restart node</DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            disabled={!node.enabled}
            onClick={() => selectAction("disable")}
          >
            Disable node
          </DropdownMenuItem>
          <DropdownMenuItem variant="destructive" onClick={() => selectAction("delete")}>
            Delete node
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <ConfirmationDialog
        open={isConfirmationOpen}
        title={`${details.verb} ${node.name}?`}
        description={details.description}
        confirmLabel={`${details.verb} node`}
        pendingLabel={details.pendingLabel}
        destructive={action !== "restart"}
        isPending={isPending}
        error={error}
        onConfirm={confirmAction}
        onOpenChange={(open) => {
          if (!open) {
            nodesStore.clearNodeActionError(node.id)
          }
          setIsConfirmationOpen(open)
        }}
      />
    </>
  )
})
