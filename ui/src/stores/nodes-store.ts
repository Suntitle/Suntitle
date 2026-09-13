import { mapNode } from "./resource-models"
import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type {
  Node,
  NodeCreate,
  NodeResource,
  NodeList,
  HostList,
  HostResource,
  NodeStatus,
  NodeType,
  NodeTypeDefinition,
  NodeTypeDefinitionList,
  NodeUpdate,
} from "./models"

export class NodesStore {
  private hosts: HostResource[] = []
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
      const [response, hosts] = await Promise.all([
        this.api.get<NodeList>("/nodes"),
        this.api.get<HostList>("/hosts"),
      ])
      runInAction(() => {
        this.hosts = hosts.items
        this.items = response.items.map((resource) => mapNode(resource, this.hosts))
          .filter((node) => (!this.typeFilter || node.type === this.typeFilter) && (!this.statusFilter || node.status === this.statusFilter))
        this.total = this.items.length
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
      const node = mapNode(await this.api.get<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}`), this.hosts)
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
      const node = mapNode(await this.api.put<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}`, this.specUpdate(nodeId, update)), this.hosts)
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
      const host = this.hosts.find((host) => host.metadata.uid === hostId)
      if (!host) throw new Error("Host is no longer available")
      const { inputNodeId, type, settings, ...fields } = create
      const node = mapNode(await this.api.post<NodeResource>(`/hosts/${encodeURIComponent(hostId)}/nodes`, {
        spec: { ...fields, hostRef: { name: host.metadata.name },
          inputRef: inputNodeId ? { name: inputNodeId } : null, settings: { [type]: settings } },
      }), this.hosts)
      runInAction(() => {
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
      const node = mapNode(await this.api.post<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}/restart`), this.hosts)
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
      const updatedNode = mapNode(await this.api.put<NodeResource>(`/nodes/${encodeURIComponent(node.id)}`, { spec: { ...node.spec, enabled: false } }), this.hosts)
      runInAction(() => { this.upsertNode(updatedNode) })
      return updatedNode
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(node.id, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(node.id) })
    }
  }

  async deleteNode(nodeId: string): Promise<void> {
    if (this.isNodeActionPending(nodeId)) return
    this.pendingNodeActionIds.add(nodeId)
    this.nodeActionErrors.delete(nodeId)
    try {
      await this.api.delete(`/nodes/${encodeURIComponent(nodeId)}`)
      await this.loadNodes()
      runInAction(() => {
        if (this.selectedNode?.id === nodeId) this.selectedNode = null
      })
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(nodeId, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(nodeId) })
    }
  }

  private specUpdate(nodeId: string, update: NodeUpdate) {
    const node = this.nodeById.get(nodeId)
    if (!node) throw new Error("Node is no longer available")
    const { inputNodeId, type, settings, ...fields } = update
    return { spec: { ...node.spec, ...fields,
      inputRef: inputNodeId ? { name: inputNodeId } : null, settings: { [type]: settings } } }
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
