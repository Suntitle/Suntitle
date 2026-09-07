import { useEffect, useRef, useState } from "react"
import { Server } from "lucide-react"
import { observer } from "mobx-react-lite"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { HostManagementDialog } from "@/components/host-management-dialog"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { cn } from "@/lib/utils"
import { useStores } from "@/stores"
import type { Host, HostStatus } from "@/stores/models"

const statusStyles: Record<HostStatus, string> = {
  online: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  offline: "border-neutral-200 bg-neutral-100 text-neutral-600",
}

function formatLastSeen(value: string) {
  const date = new Date(value)

  if (Number.isNaN(date.getTime())) return "Unknown"

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date)
}

export const HostsPage = observer(function HostsPage() {
  const { hostsStore } = useStores()
  const requestedHosts = useRef(false)
  const [selectedHost, setSelectedHost] = useState<Host | null>(null)
  const [isHostDialogOpen, setIsHostDialogOpen] = useState(false)

  useEffect(() => {
    if (requestedHosts.current) return
    requestedHosts.current = true
    void hostsStore.loadHosts().catch(() => undefined)
  }, [hostsStore])

  function openHost(host: Host) {
    setSelectedHost(host)
    setIsHostDialogOpen(true)
  }

  return (
    <>
      <section className="min-w-0 flex-1 px-5 py-8 sm:px-8 md:px-10 md:py-10 lg:px-12 lg:py-12">
        <div className="mx-auto w-full max-w-6xl">
          <header>
            <p className="text-xs font-semibold uppercase tracking-[0.18em] text-violet-600">Infrastructure</p>
            <h1 className="mt-2 text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">Hosts</h1>
            <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground sm:text-base">
              View the machines available to your live subtitle workspace.
            </p>
          </header>

          <Card className="mt-8 overflow-hidden border-violet-100/80 bg-white shadow-xl shadow-violet-950/[.06]">
            <CardHeader className="flex flex-row items-start justify-between gap-4 border-b border-violet-100/80 bg-gradient-to-r from-white to-violet-50/50 px-5 py-5 sm:px-6">
              <div>
                <CardTitle className="text-base tracking-[-0.015em] sm:text-lg">Available hosts</CardTitle>
                <CardDescription className="mt-1">Machines currently registered with Suntitle.</CardDescription>
              </div>
              <Badge variant="outline" className="border-violet-200 bg-violet-50 px-2.5 py-1 text-violet-700">
                {hostsStore.total} {hostsStore.total === 1 ? "host" : "hosts"}
              </Badge>
            </CardHeader>
            <CardContent className="p-0">
              <Table>
                <TableHeader className="bg-violet-50/60">
                  <TableRow className="hover:bg-violet-50/60">
                    <TableHead className="pl-5 sm:pl-6">Host</TableHead>
                    <TableHead>IP address</TableHead>
                    <TableHead>Version</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead className="pr-5 sm:pr-6">Last seen</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {hostsStore.isLoading && hostsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent">
                      <TableCell colSpan={5} className="h-24 text-center text-muted-foreground">Loading hosts…</TableCell>
                    </TableRow>
                  )}

                  {hostsStore.error && hostsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent">
                      <TableCell colSpan={5} className="h-24 text-center text-destructive">{hostsStore.error}</TableCell>
                    </TableRow>
                  )}

                  {!hostsStore.isLoading && !hostsStore.error && hostsStore.items.length === 0 && (
                    <TableRow className="hover:bg-transparent">
                      <TableCell colSpan={5} className="h-24 text-center text-muted-foreground">No hosts are available yet.</TableCell>
                    </TableRow>
                  )}

                  {hostsStore.items.map((host) => (
                    <TableRow key={host.id} className="hover:bg-violet-50/35">
                      <TableCell className="pl-5 font-medium sm:pl-6">
                        <span className="flex items-center gap-3">
                          <span className="grid size-8 place-items-center rounded-lg bg-violet-100 text-violet-700">
                            <Server className="size-4" aria-hidden="true" />
                          </span>
                          <Button
                            type="button"
                            variant="ghost"
                            className="-ml-2 h-auto px-2 py-1 font-medium hover:bg-violet-100/70"
                            onClick={() => openHost(host)}
                          >
                            {host.name}
                          </Button>
                        </span>
                      </TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">{host.ip}</TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">{host.version}</TableCell>
                      <TableCell>
                        <Badge variant="outline" className={cn("capitalize", statusStyles[host.status])}>
                          <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
                          {host.status}
                        </Badge>
                      </TableCell>
                      <TableCell className="pr-5 text-muted-foreground sm:pr-6">{formatLastSeen(host.lastSeenAt)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </div>
      </section>

      <HostManagementDialog host={selectedHost} open={isHostDialogOpen} onOpenChange={setIsHostDialogOpen} />
    </>
  )
})
