import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type { Node, NodeList, NodeStatus, NodeType, NodeUpdate } from "./models"

export class NodesStore {
  items: Node[] = []
  total = 0
  selectedNode: Node | null = null
  typeFilter: NodeType | null = null
  statusFilter: NodeStatus | null = null
  isLoading = false
  isSaving = false
  error: string | null = null

  constructor(private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  get enabledNodes(): Node[] { return this.items.filter((node) => node.enabled) }
  get nodeById(): ReadonlyMap<string, Node> { return new Map(this.items.map((node) => [node.id, node])) }

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
