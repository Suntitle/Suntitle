import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type {
  Node,
  NodeCreate,
  NodeList,
  NodeStatus,
  NodeType,
  NodeTypeDefinition,
  NodeTypeDefinitionList,
  NodeUpdate,
} from "./models"

function getInputNodeId(node: Node, nodes: Node[]): string | null {
  const nodeIds = new Set(nodes.map((item) => item.id))

  for (const candidate of nodes) {
    for (const connection of candidate.connections) {
      if (!nodeIds.has(connection.peerNodeId)) continue

      const sourceId = connection.direction === "outgoing" ? candidate.id : connection.peerNodeId
      const targetId = connection.direction === "outgoing" ? connection.peerNodeId : candidate.id
      if (targetId === node.id) return sourceId
    }
  }

  return null
}

function createEnabledUpdate(node: Node, nodes: Node[], enabled: boolean): NodeUpdate {
  const base = {
    name: node.name,
    description: node.description ?? null,
    enabled,
    inputNodeId: getInputNodeId(node, nodes),
  }

  switch (node.type) {
    case "audio_source": return { ...base, type: node.type, settings: node.settings }
    case "transcriber": return { ...base, type: node.type, settings: node.settings }
    case "translator": return { ...base, type: node.type, settings: node.settings }
    case "display": return { ...base, type: node.type, settings: node.settings }
  }
}

export class NodesStore {
  items: Node[] = []
  total = 0
  nodeTypes: NodeTypeDefinition[] = []
  nodeTypesTotal = 0
  selectedNode: Node | null = null
  typeFilter: NodeType | null = null
  statusFilter: NodeStatus | null = null
  isLoading = false
  isLoadingNodeTypes = false
  isSaving = false
  isInstalling = false
  pendingNodeActionIds = new Set<string>()
  error: string | null = null
  nodeTypesError: string | null = null
  installError: string | null = null
  saveError: string | null = null
  nodeActionErrors = new Map<string, string>()

  constructor(private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  get enabledNodes(): Node[] { return this.items.filter((node) => node.enabled) }
  get nodeById(): ReadonlyMap<string, Node> { return new Map(this.items.map((node) => [node.id, node])) }

  nodesForHost(hostId: string): Node[] {
    return this.items.filter((node) => node.host.id === hostId)
  }

  get nodesByType(): ReadonlyMap<NodeType, Node[]> {
    const groups = new Map<NodeType, Node[]>([
      ["audio_source", []], ["transcriber", []], ["translator", []], ["display", []],
    ])
    this.items.forEach((node) => groups.get(node.type)?.push(node))
    return groups
  }

  setTypeFilter(type: NodeType | null): void { this.typeFilter = type }
  setStatusFilter(status: NodeStatus | null): void { this.statusFilter = status }
  selectNode(node: Node | null): void { this.selectedNode = node }
  clearError(): void { this.error = null }
  clearInstallError(): void { this.installError = null }
  clearSaveError(): void { this.saveError = null }
  isNodeActionPending(nodeId: string): boolean { return this.pendingNodeActionIds.has(nodeId) }
  nodeActionError(nodeId: string): string | null { return this.nodeActionErrors.get(nodeId) ?? null }
  clearNodeActionError(nodeId: string): void { this.nodeActionErrors.delete(nodeId) }

  async loadNodeTypes(): Promise<void> {
    this.isLoadingNodeTypes = true
    this.nodeTypesError = null
    try {
      const response = await this.api.get<NodeTypeDefinitionList>("/node-types")
      runInAction(() => {
        this.nodeTypes = response.items
        this.nodeTypesTotal = response.total
      })
    } catch (error) {
      runInAction(() => { this.nodeTypesError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoadingNodeTypes = false })
    }
  }

  async loadNodes(): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      const params = new URLSearchParams()
      if (this.typeFilter) params.set("type", this.typeFilter)
      if (this.statusFilter) params.set("status", this.statusFilter)
      const query = params.size > 0 ? `?${params.toString()}` : ""
      const response = await this.api.get<NodeList>(`/nodes${query}`)
      runInAction(() => {
        this.items = response.items
        this.total = response.total
      })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async loadNode(nodeId: string): Promise<Node> {
    this.isLoading = true
    this.error = null
    try {
      const node = await this.api.get<Node>(`/nodes/${encodeURIComponent(nodeId)}`)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async updateNode(nodeId: string, update: NodeUpdate): Promise<Node> {
    this.isSaving = true
    this.saveError = null
    try {
      const node = await this.api.put<Node>(`/nodes/${encodeURIComponent(nodeId)}`, update)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.saveError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isSaving = false })
    }
  }

  async installNode(hostId: string, create: NodeCreate): Promise<Node> {
    this.isInstalling = true
    this.installError = null
    try {
      const node = await this.api.post<Node>(`/hosts/${encodeURIComponent(hostId)}/nodes`, create)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.installError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isInstalling = false })
    }
  }

  async restartNode(nodeId: string): Promise<Node> {
    this.pendingNodeActionIds.add(nodeId)
    this.nodeActionErrors.delete(nodeId)
    try {
      const node = await this.api.post<Node>(`/nodes/${encodeURIComponent(nodeId)}/restart`)
      runInAction(() => { this.upsertNode(node) })
      return node
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(nodeId, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(nodeId) })
    }
  }

  async disableNode(node: Node): Promise<Node> {
    this.pendingNodeActionIds.add(node.id)
    this.nodeActionErrors.delete(node.id)
    try {
      const update = createEnabledUpdate(node, this.items, false)
      const updatedNode = await this.api.put<Node>(`/nodes/${encodeURIComponent(node.id)}`, update)
      runInAction(() => { this.upsertNode(updatedNode) })
      return updatedNode
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(node.id, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(node.id) })
    }
  }

  private upsertNode(node: Node): void {
    const index = this.items.findIndex((item) => item.id === node.id)
    if (index === -1) {
      this.items = [...this.items, node]
      this.total += 1
    } else {
      this.items = this.items.map((item) => item.id === node.id ? node : item)
    }
  }
}
