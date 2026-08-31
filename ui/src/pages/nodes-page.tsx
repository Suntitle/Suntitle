import { useEffect, useMemo, useRef } from "react"
import { AudioWaveform, Languages, Mic, Monitor } from "lucide-react"
import { observer } from "mobx-react-lite"

import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { cn } from "@/lib/utils"
import { useStores } from "@/stores"
import type { DataType, Node, NodeStatus, NodeType } from "@/stores/models"

const NODE_WIDTH = 210
const NODE_HEIGHT = 196
const HORIZONTAL_GAP = 70
const VERTICAL_GAP = 32
const CANVAS_TOP_PADDING = 48

interface TopologyEdge {
  id: string
  sourceId: string
  targetId: string
  dataType: DataType
}

interface Pipeline {
  id: string
  nodes: Node[]
  edges: TopologyEdge[]
}

interface PositionedPipeline extends Pipeline {
  width: number
  height: number
  positions: ReadonlyMap<string, { x: number; y: number }>
}

const nodeTypeLabels: Record<NodeType, string> = {
  audio_source: "Audio source",
  transcriber: "Transcriber",
  translator: "Translator",
  display: "Display",
}

const nodeTypeIcons: Record<NodeType, typeof Mic> = {
  audio_source: Mic,
  transcriber: AudioWaveform,
  translator: Languages,
  display: Monitor,
}

const statusStyles: Record<NodeStatus, string> = {
  connected: "border-emerald-200 bg-emerald-50 text-emerald-700",
  degraded: "border-amber-200 bg-amber-50 text-amber-700",
  disconnected: "border-neutral-200 bg-neutral-100 text-neutral-600",
}

function buildEdges(nodes: Node[]): TopologyEdge[] {
  const nodeIds = new Set(nodes.map((node) => node.id))
  const edges = new Map<string, TopologyEdge>()

  nodes.forEach((node) => {
    node.connections.forEach((connection) => {
      if (!nodeIds.has(connection.peerNodeId)) return

      const sourceId = connection.direction === "outgoing" ? node.id : connection.peerNodeId
      const targetId = connection.direction === "outgoing" ? connection.peerNodeId : node.id
      const key = `${sourceId}:${targetId}:${connection.dataType}`

      if (!edges.has(key)) {
        edges.set(key, {
          id: connection.id,
          sourceId,
          targetId,
          dataType: connection.dataType,
        })
      }
    })
  })

  return [...edges.values()]
}

function buildPipelines(nodes: Node[]): Pipeline[] {
  const edges = buildEdges(nodes)
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const adjacency = new Map(nodes.map((node) => [node.id, new Set<string>()]))

  edges.forEach((edge) => {
    adjacency.get(edge.sourceId)?.add(edge.targetId)
    adjacency.get(edge.targetId)?.add(edge.sourceId)
  })

  const visited = new Set<string>()
  const pipelines: Pipeline[] = []

  nodes.forEach((node) => {
    if (visited.has(node.id)) return

    const memberIds = new Set<string>()
    const pending = [node.id]
    visited.add(node.id)

    while (pending.length > 0) {
      const currentId = pending.pop()
      if (!currentId) continue

      memberIds.add(currentId)
      adjacency.get(currentId)?.forEach((neighborId) => {
        if (visited.has(neighborId)) return
        visited.add(neighborId)
        pending.push(neighborId)
      })
    }

    const pipelineNodes = [...memberIds]
      .map((id) => nodeById.get(id))
      .filter((item): item is Node => item !== undefined)

    pipelines.push({
      id: pipelineNodes.map((item) => item.id).sort().join("-"),
      nodes: pipelineNodes,
      edges: edges.filter((edge) => memberIds.has(edge.sourceId) && memberIds.has(edge.targetId)),
    })
  })

  return pipelines.sort((left, right) => right.nodes.length - left.nodes.length || left.id.localeCompare(right.id))
}

function positionPipeline(pipeline: Pipeline): PositionedPipeline {
  const outgoing = new Map(pipeline.nodes.map((node) => [node.id, [] as string[]]))
  const indegree = new Map(pipeline.nodes.map((node) => [node.id, 0]))

  pipeline.edges.forEach((edge) => {
    outgoing.get(edge.sourceId)?.push(edge.targetId)
    indegree.set(edge.targetId, (indegree.get(edge.targetId) ?? 0) + 1)
  })

  const rank = new Map(pipeline.nodes.map((node) => [node.id, 0]))
  const queue = pipeline.nodes
    .filter((node) => indegree.get(node.id) === 0)
    .sort((left, right) => left.name.localeCompare(right.name))
  const processed = new Set<string>()

  while (queue.length > 0) {
    const node = queue.shift()
    if (!node) continue

    processed.add(node.id)
    outgoing.get(node.id)?.forEach((targetId) => {
      rank.set(targetId, Math.max(rank.get(targetId) ?? 0, (rank.get(node.id) ?? 0) + 1))
      indegree.set(targetId, (indegree.get(targetId) ?? 0) - 1)

      if (indegree.get(targetId) === 0) {
        const target = pipeline.nodes.find((item) => item.id === targetId)
        if (target) {
          queue.push(target)
          queue.sort((left, right) => left.name.localeCompare(right.name))
        }
      }
    })
  }

  if (processed.size !== pipeline.nodes.length) {
    pipeline.nodes
      .slice()
      .sort((left, right) => left.name.localeCompare(right.name))
      .forEach((node, index) => rank.set(node.id, index))
  }

  const columns = new Map<number, Node[]>()
  pipeline.nodes.forEach((node) => {
    const nodeRank = rank.get(node.id) ?? 0
    const column = columns.get(nodeRank) ?? []
    column.push(node)
    column.sort((left, right) => left.name.localeCompare(right.name))
    columns.set(nodeRank, column)
  })

  const positions = new Map<string, { x: number; y: number }>()
  let maximumRank = 0
  let maximumRows = 1

  columns.forEach((column, columnRank) => {
    maximumRank = Math.max(maximumRank, columnRank)
    maximumRows = Math.max(maximumRows, column.length)
    column.forEach((node, row) => {
      positions.set(node.id, {
        x: columnRank * (NODE_WIDTH + HORIZONTAL_GAP),
        y: CANVAS_TOP_PADDING + row * (NODE_HEIGHT + VERTICAL_GAP),
      })
    })
  })

  return {
    ...pipeline,
    positions,
    width: maximumRank * (NODE_WIDTH + HORIZONTAL_GAP) + NODE_WIDTH,
    height: CANVAS_TOP_PADDING + maximumRows * NODE_HEIGHT + (maximumRows - 1) * VERTICAL_GAP,
  }
}

function getPipelineLabel(index: number) {
  return index < 26 ? `Pipeline ${String.fromCharCode(65 + index)}` : `Pipeline ${index + 1}`
}

export function NodeCard({ node, position }: { node: Node; position: { x: number; y: number } }) {
  const Icon = nodeTypeIcons[node.type]

  return (
    <Card
      data-testid="topology-node"
      aria-label={`${node.name}, ${nodeTypeLabels[node.type]}, ${node.status}`}
      className="absolute h-[196px] w-[210px] gap-0 border-violet-100/80 bg-white py-0 shadow-sm"
      style={{ left: position.x, top: position.y }}
    >
      <CardContent className="flex h-full flex-col px-5 py-5">
        <div className="flex items-center gap-2.5">
          <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-violet-50 text-violet-700">
            <Icon className="size-[18px]" aria-hidden="true" />
          </span>
          <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-violet-700">
            {nodeTypeLabels[node.type]}
          </span>
        </div>
        <h3 className="mt-3 truncate text-base font-semibold tracking-[-0.015em]">{node.name}</h3>
        <p className="mt-2 truncate text-xs text-muted-foreground">{node.host.name}</p>
        <Badge variant="outline" className={cn("mt-auto capitalize", statusStyles[node.status])}>
          {node.status}
        </Badge>
      </CardContent>
    </Card>
  )
}

export function PipelineDiagram({ pipeline, index }: { pipeline: PositionedPipeline; index: number }) {
  const label = getPipelineLabel(index)
  const markerId = `arrow-${index}-${pipeline.id.replaceAll(/[^a-zA-Z0-9_-]/g, "")}`

  return (
    <section data-testid="topology-pipeline" aria-labelledby={`pipeline-${index}-title`}>
      <h2 id={`pipeline-${index}-title`} className="mb-2 text-xs font-medium text-muted-foreground">{label}</h2>
      <div className="overflow-x-auto pb-2">
        <div className="relative" style={{ width: Math.max(pipeline.width, NODE_WIDTH), height: pipeline.height }}>
          <svg className="pointer-events-none absolute inset-0 size-full overflow-visible text-violet-600" aria-hidden="true">
            <defs>
              <marker id={markerId} markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto" markerUnits="strokeWidth">
                <path d="M0,0 L8,4 L0,8 Z" fill="currentColor" />
              </marker>
            </defs>
            {pipeline.edges.map((edge) => {
              const source = pipeline.positions.get(edge.sourceId)
              const target = pipeline.positions.get(edge.targetId)
              if (!source || !target) return null

              const sourceX = source.x + NODE_WIDTH
              const sourceY = source.y + NODE_HEIGHT / 2
              const targetX = target.x
              const targetY = target.y + NODE_HEIGHT / 2
              const controlOffset = Math.max(24, Math.abs(targetX - sourceX) / 2)

              return (
                <path
                  key={edge.id}
                  data-testid="topology-connection"
                  d={`M ${sourceX} ${sourceY} C ${sourceX + controlOffset} ${sourceY}, ${targetX - controlOffset} ${targetY}, ${targetX} ${targetY}`}
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  markerEnd={`url(#${markerId})`}
                />
              )
            })}
          </svg>

          {pipeline.edges.map((edge) => {
            const source = pipeline.positions.get(edge.sourceId)
            const target = pipeline.positions.get(edge.targetId)
            if (!source || !target) return null

            return (
              <Badge
                key={`${edge.id}-label`}
                variant="outline"
                className="absolute z-10 -translate-x-1/2 border-violet-200 bg-white text-violet-700"
                style={{
                  left: (source.x + NODE_WIDTH + target.x) / 2,
                  top: (source.y + target.y) / 2 + NODE_HEIGHT / 2 - 128,
                }}
              >
                {edge.dataType}
              </Badge>
            )
          })}

          {pipeline.nodes.map((node) => {
            const position = pipeline.positions.get(node.id)
            return position ? <NodeCard key={node.id} node={node} position={position} /> : null
          })}
        </div>
      </div>
    </section>
  )
}

// The observer wrapper is the page component export used by the route registry.
// oxlint-disable-next-line react/only-export-components
export const NodesPage = observer(function NodesPage() {
  const { nodesStore } = useStores()
  const requestedNodes = useRef(false)

  useEffect(() => {
    if (requestedNodes.current) return
    requestedNodes.current = true
    void nodesStore.loadNodes().catch(() => undefined)
  }, [nodesStore])

  const pipelines = useMemo(
    () => buildPipelines(nodesStore.items).map(positionPipeline),
    [nodesStore.items],
  )

  return (
    <section className="min-w-0 flex-1 px-5 py-8 sm:px-8 md:px-10 md:py-10 lg:px-12 lg:py-12">
      <div className="mx-auto w-full max-w-6xl">
        <header>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-violet-600">Infrastructure</p>
          <h1 className="mt-2 text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">Nodes</h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground sm:text-base">
            View the live subtitle pipeline and how data moves between nodes.
          </p>
        </header>

        <Card className="mt-8 overflow-hidden border-violet-100/80 bg-white shadow-xl shadow-violet-950/[.06]">
          <CardHeader className="flex flex-row items-start justify-between gap-4 border-b border-violet-100/80 bg-gradient-to-r from-white to-violet-50/50 px-5 py-5 sm:px-6">
            <div>
              <CardTitle className="text-base tracking-[-0.015em] sm:text-lg">Pipeline topology</CardTitle>
              <CardDescription className="mt-1">Independent pipelines and the data flowing through each one.</CardDescription>
            </div>
            <Badge variant="outline" className="border-violet-200 bg-violet-50 px-2.5 py-1 text-violet-700">
              {nodesStore.total} {nodesStore.total === 1 ? "node" : "nodes"}
            </Badge>
          </CardHeader>
          <CardContent className="bg-violet-50/30 px-5 py-6 sm:px-6">
            {nodesStore.isLoading && nodesStore.items.length === 0 && (
              <div className="grid min-h-72 place-items-center text-sm text-muted-foreground">Loading topology…</div>
            )}

            {nodesStore.error && nodesStore.items.length === 0 && (
              <div role="alert" className="grid min-h-72 place-items-center text-sm text-destructive">{nodesStore.error}</div>
            )}

            {!nodesStore.isLoading && !nodesStore.error && nodesStore.items.length === 0 && (
              <div className="grid min-h-72 place-items-center text-sm text-muted-foreground">No nodes are available yet.</div>
            )}

            {pipelines.length > 0 && (
              <div className="space-y-8">
                {pipelines.map((pipeline, index) => <PipelineDiagram key={pipeline.id} pipeline={pipeline} index={index} />)}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </section>
  )
})
