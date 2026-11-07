const nodeId = "30000000-0000-4000-8000-000000000001"
const nodeName = "Studio microphone"
const accessToken = "mock-token"

function openDeleteMenu() {
  cy.get(`button[aria-label="Actions for ${nodeName}"]`).click()
  cy.contains('[data-slot="dropdown-menu-item"]', "Delete node").click()
  cy.get('[data-slot="alert-dialog-title"]').should("have.text", `Delete ${nodeName}?`)
}

function visitWorkspace(location: "nodes" | "host") {
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" }).as("getHosts")
  cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" }).as("getNodeTypes")
  cy.visit(location === "nodes" ? "/nodes" : "/hosts", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
  cy.wait(["@getCurrentUser", "@getHosts"])
  if (location === "host") {
    cy.contains("button", "studio-host").click()
    cy.wait("@getNodeTypes")
  }
  cy.wait("@getNodes")
}

describe("Node deletion", () => {
  for (const location of ["nodes", "host"] as const) {
    it(`confirms deletion of a disabled node from ${location}, blocks duplicate requests, and refreshes the list`, () => {
      let deleted = false
      let releaseDelete: () => void
      const deleteResponse = new Cypress.Promise<void>((resolve) => { releaseDelete = resolve })
      cy.fixture("api/v1/nodes.json").then((body) => {
        body.items.find((node: { name: string }) => node.name === nodeId).enabled = false
        cy.intercept("GET", "**/api/v1/nodes", (request) => {
          const items = deleted
            ? body.items.filter((node: { name: string }) => node.name !== nodeId)
            : body.items
          request.reply({ body: { ...body, items, total: items.length } })
        }).as("getNodes")
      })
      cy.intercept("DELETE", `**/api/v1/nodes/${nodeId}`, (request) => {
        return deleteResponse.then(() => {
          deleted = true
          request.reply({ statusCode: 202, body: "" })
        })
      }).as("deleteNode")

      visitWorkspace(location)
      openDeleteMenu()
      cy.contains('[data-slot="alert-dialog-description"]', "This cannot be undone.").should("be.visible")
      cy.get('[data-slot="alert-dialog-cancel"]').click()
      cy.get('[data-slot="alert-dialog-content"]').should("not.exist")
      cy.get("@deleteNode.all").should("have.length", 0)

      openDeleteMenu()
      cy.get('[data-slot="alert-dialog-action"]').click()
      cy.contains('[data-slot="alert-dialog-action"]', "Deleting…").should("be.disabled")
      cy.get('[data-slot="alert-dialog-cancel"]').should("be.disabled")
      cy.get("body").type("{esc}")
      cy.get('[data-slot="alert-dialog-content"]').should("be.visible")
        .then(() => releaseDelete())
      cy.wait("@deleteNode").its("request.headers.authorization").should("equal", `Bearer ${accessToken}`)
      cy.wait("@getNodes")
      cy.get("@deleteNode.all").should("have.length", 1)
      cy.get('[data-slot="alert-dialog-content"]').should("not.exist")
      cy.get(`button[aria-label="Actions for ${nodeName}"]`).should("not.exist")
      cy.contains("6 nodes").should("be.visible")
      if (location === "nodes") {
        cy.get('[data-testid="topology-node"]').should("have.length", 6)
        cy.get('[data-testid="topology-connection"]').should("have.length", 4)
      } else {
        cy.get('[data-slot="dialog-title"]').should("have.text", "Manage studio-host")
        cy.get('button[aria-label="Input"]').click()
        cy.get('[data-slot="select-content"]').should("not.contain.text", nodeName)
      }
    })

    it(`keeps the node and allows retry after a deletion error from ${location}`, () => {
      cy.intercept("GET", "**/api/v1/nodes", { fixture: "api/v1/nodes.json" }).as("getNodes")
      cy.intercept("DELETE", `**/api/v1/nodes/${nodeId}`, {
        statusCode: 503,
        body: { error: { code: "Unavailable", message: "Could not delete this node. Try again." } },
      }).as("failedDelete")
      visitWorkspace(location)
      openDeleteMenu()
      cy.get('[data-slot="alert-dialog-action"]').click()
      cy.wait("@failedDelete")
      cy.get('[data-slot="alert-dialog-content"] [role="alert"]')
        .should("have.text", "Could not delete this node. Try again.")
      cy.get('[data-slot="alert-dialog-action"]').should("be.enabled")
      cy.get(`button[aria-label="Actions for ${nodeName}"]`).should("exist")
      cy.intercept("DELETE", `**/api/v1/nodes/${nodeId}`, { statusCode: 202, body: "" }).as("retryDelete")
      cy.intercept("GET", "**/api/v1/nodes", { body: { items: [], total: 0 } }).as("emptyNodes")
      cy.get('[data-slot="alert-dialog-action"]').click()
      cy.wait(["@retryDelete", "@emptyNodes"])
      cy.get('[data-slot="alert-dialog-content"]').should("not.exist")
      cy.contains("0 nodes").scrollIntoView().should("be.visible")
      cy.contains(location === "nodes" ? "No nodes" : "No nodes are installed on this host.").should("be.visible")
    })
  }

  it("keeps terminating nodes visible until the API reports their removal", () => {
    cy.fixture("api/v1/nodes.json").then((body) => {
      body.items[0].status.phase = "Terminating"
      cy.intercept("GET", "**/api/v1/nodes", { body }).as("getNodes")
    })
    cy.intercept("DELETE", `**/api/v1/nodes/${nodeId}`, { statusCode: 202, body: "" }).as("deleteNode")
    visitWorkspace("nodes")
    openDeleteMenu()
    cy.get('[data-slot="alert-dialog-action"]').click()
    cy.wait(["@deleteNode", "@getNodes"])
    cy.get('[data-slot="alert-dialog-content"]').should("not.exist")
    cy.contains('[data-testid="topology-node"]', nodeName).should("be.visible")
    cy.contains("7 nodes").should("be.visible")
  })
})
