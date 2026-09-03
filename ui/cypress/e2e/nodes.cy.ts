const accessToken = "mock-token"

function visitAuthenticatedWorkspace() {
  cy.intercept("GET", "**/api/v1/auth/me", {
    fixture: "api/v1/auth/me.json",
  }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/hosts", {
    fixture: "api/v1/hosts.json",
  }).as("getHosts")
  cy.intercept("GET", "**/api/v1/nodes", {
    fixture: "api/v1/nodes.json",
  }).as("getNodes")

  cy.visit("/", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Nodes page", () => {
  it("uses the Nodes route and renders unrelated pipelines separately", () => {
    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])

    cy.contains("a", "Nodes").click()
    cy.location("pathname").should("equal", "/nodes")
    cy.wait("@getNodes")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)

    cy.get("h1").should("have.text", "Nodes").and("be.visible")
    cy.contains("7 nodes").should("be.visible")
    cy.get('[data-testid="topology-pipeline"]').should("have.length", 2)

    cy.get('[data-testid="topology-pipeline"]').eq(0).within(() => {
      cy.contains("Pipeline A").should("be.visible")
      cy.contains("Studio microphone").should("be.visible")
      cy.contains("Live transcriber").should("be.visible")
      cy.contains("English → Spanish").should("exist")
      cy.contains("Stage display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 3)
    })

    cy.get('[data-testid="topology-pipeline"]').eq(1).within(() => {
      cy.contains("Pipeline B").should("be.visible")
      cy.contains("Lobby microphone").should("be.visible")
      cy.contains("Lobby transcriber").should("be.visible")
      cy.contains("Lobby display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 2)
    })

    cy.reload()
    cy.wait(["@getCurrentUser", "@getNodes"])
    cy.location("pathname").should("equal", "/nodes")
    cy.get("h1").should("have.text", "Nodes")
  })

  it("opens node settings, filters compatible inputs, and updates the node", () => {
    cy.intercept("GET", "**/api/v1/node-types", {
      fixture: "api/v1/node-types.json",
    }).as("getNodeTypes")
    cy.intercept("PUT", "**/api/v1/nodes/30000000-0000-4000-8000-000000000003", {
      fixture: "api/v1/nodes/30000000-0000-4000-8000-000000000003.json",
    }).as("updateNode")

    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])

    cy.contains("a", "Nodes").click()
    cy.wait("@getNodes")

    cy.contains('[data-testid="topology-node"]', "Live transcriber").click()
    cy.wait("@getNodeTypes")

    cy.get('[data-slot="dialog-title"]').should("have.text", "Update Live transcriber")
    cy.contains("section", "Node details").within(() => {
      cy.contains("Transcriber").should("be.visible")
      cy.contains("studio-host").should("be.visible")
      cy.contains("connected").should("be.visible")
    })
    cy.contains("section", "Current input").within(() => {
      cy.contains("Studio microphone").should("be.visible")
      cy.contains("Audio source · studio-host").should("be.visible")
    })

    cy.get('button[aria-label="Input"]').should("contain.text", "Studio microphone · studio-host").click()
    cy.get('[data-slot="select-content"]').within(() => {
      cy.contains('[data-slot="select-item"]', "Studio microphone · studio-host").should("be.visible")
      cy.contains('[data-slot="select-item"]', "Lobby microphone · studio-host").should("be.visible")
      cy.contains("Live transcriber").should("not.exist")
      cy.contains("English → Spanish").should("not.exist")
    })
    cy.get('[data-slot="select-item"]').contains("Studio microphone · studio-host").click()

    cy.contains("Accepts: Audio source").should("be.visible")
    cy.get("#node-name").clear().type("Updated transcriber")
    cy.get("#node-setting-model").clear().type("medium")
    cy.contains("button", "Save changes").click()

    cy.wait("@updateNode").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body).to.deep.equal({
        type: "transcriber",
        name: "Updated transcriber",
        description: "English transcription",
        enabled: true,
        inputNodeId: "30000000-0000-4000-8000-000000000001",
        settings: {
          model: "medium",
          language: "en-US",
          task: "transcribe",
          beamSize: 5,
          vadEnabled: true,
        },
      })
    })

    cy.get('[data-slot="dialog-content"]').should("not.exist")
    cy.contains('[data-testid="topology-node"]', "Updated transcriber").should("be.visible")
  })
})
