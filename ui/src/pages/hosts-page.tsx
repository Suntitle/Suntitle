import { useEffect, useRef } from "react"
import { Box, Boxes, CircleUserRound, Server } from "lucide-react"
import { observer } from "mobx-react-lite"

import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { cn } from "@/lib/utils"
import { useStores } from "@/stores"
import type { HostStatus } from "@/stores/models"

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

  useEffect(() => {
    if (requestedHosts.current) return
    requestedHosts.current = true
    void hostsStore.loadHosts().catch(() => undefined)
  }, [hostsStore])

  return (
    <main className="flex min-h-svh flex-col bg-violet-50/70 text-foreground md:flex-row">
      <aside className="flex border-b border-neutral-800 bg-neutral-950 px-5 py-4 text-neutral-300 md:min-h-svh md:w-64 md:flex-col md:border-b-0 md:px-4 md:py-6">
        <div className="flex min-w-0 items-center gap-3 md:px-2">
          <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-violet-600 text-white shadow-lg shadow-violet-950/30">
            <Box className="size-5" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-base font-semibold tracking-[-0.025em] text-white">Suntitle</p>
            <p className="mt-0.5 text-[10px] font-semibold uppercase tracking-[0.2em] text-neutral-500">Live subtitles</p>
          </div>
        </div>

        <nav aria-label="Primary navigation" className="ml-auto flex items-center gap-1 md:ml-0 md:mt-10 md:grid md:gap-1">
          <span aria-current="page" className="flex items-center gap-3 rounded-lg bg-violet-600 px-3 py-2.5 text-sm font-medium text-white shadow-sm shadow-violet-950/30">
            <Server className="size-4" aria-hidden="true" />
            <span className="hidden sm:inline">Hosts</span>
          </span>
          <span className="flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium text-neutral-400 md:flex">
            <Boxes className="size-4" aria-hidden="true" />
            <span className="hidden sm:inline">Nodes</span>
          </span>
        </nav>

        <div className="mt-auto hidden items-center gap-3 rounded-xl border border-white/[.07] bg-white/[.04] p-3 md:flex">
          <CircleUserRound className="size-8 text-violet-300" aria-hidden="true" />
          <div className="min-w-0">
            <p className="truncate text-sm font-medium text-white">Administrator</p>
            <p className="truncate text-xs text-neutral-500">Workspace owner</p>
          </div>
        </div>
      </aside>

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
                          {host.name}
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
    </main>
  )
})
