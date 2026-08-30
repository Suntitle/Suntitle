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
  error: string | null = null
  nodeTypesError: string | null = null
  installError: string | null = null

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
    this.error = null
    try {
      const node = await this.api.put<Node>(`/nodes/${encodeURIComponent(nodeId)}`, update)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
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

  private upsertNode(node: Node): void {
    const index = this.items.findIndex((item) => item.id === node.id)
    if (index === -1) {
      this.items.push(node)
      this.total += 1
    } else {
      this.items[index] = node
    }
  }
}
