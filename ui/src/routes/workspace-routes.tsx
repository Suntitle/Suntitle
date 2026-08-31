import type { ReactNode } from "react"
import { Boxes, Server } from "lucide-react"

import { HostsPage } from "@/pages/hosts-page"
import { NodesPage } from "@/pages/nodes-page"

export interface WorkspaceRoute {
  path: string
  label: string
  icon: typeof Server
  element: ReactNode
}

export const workspaceRoutes: WorkspaceRoute[] = [
  { path: "/hosts", label: "Hosts", icon: Server, element: <HostsPage /> },
  { path: "/nodes", label: "Nodes", icon: Boxes, element: <NodesPage /> },
]
