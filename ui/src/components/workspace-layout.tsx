import { Box, CircleUserRound } from "lucide-react"
import { Navigate, NavLink, Route, Routes } from "react-router-dom"

import { buttonVariants } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { workspaceRoutes } from "@/routes/workspace-routes"

export function WorkspaceLayout() {
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
          {workspaceRoutes.map((item) => {
            const Icon = item.icon

            return (
              <NavLink
                key={item.path}
                to={item.path}
                className={({ isActive }) => cn(
                  buttonVariants({ variant: "ghost" }),
                  "h-10 justify-start gap-3 px-3 text-sm font-medium text-neutral-400 hover:bg-white/[.07] hover:text-white",
                  isActive && "bg-violet-600 text-white shadow-sm shadow-violet-950/30 hover:bg-violet-600",
                )}
              >
                <Icon className="size-4" aria-hidden="true" />
                <span className="hidden sm:inline">{item.label}</span>
              </NavLink>
            )
          })}
        </nav>

        <div className="mt-auto hidden items-center gap-3 rounded-xl border border-white/[.07] bg-white/[.04] p-3 md:flex">
          <CircleUserRound className="size-8 text-violet-300" aria-hidden="true" />
          <div className="min-w-0">
            <p className="truncate text-sm font-medium text-white">Administrator</p>
            <p className="truncate text-xs text-neutral-500">Workspace owner</p>
          </div>
        </div>
      </aside>

      <Routes>
        <Route path="/" element={<Navigate to="/hosts" replace />} />
        {workspaceRoutes.map((page) => <Route key={page.path} path={page.path} element={page.element} />)}
        <Route path="*" element={<Navigate to="/hosts" replace />} />
      </Routes>
    </main>
  )
}
