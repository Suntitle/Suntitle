const accessToken = "mock-token"

function visitAuthenticatedHosts() {
  cy.intercept("GET", "**/api/v1/auth/me", {
    fixture: "api/v1/auth/me.json",
  }).as("getCurrentUser")

  cy.visit("/", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Hosts page", () => {
  it("renders hosts returned by the API", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      fixture: "api/v1/hosts.json",
    }).as("getHosts")

    visitAuthenticatedHosts()

    cy.wait("@getCurrentUser")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)
    cy.wait("@getHosts")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)

    cy.location("pathname").should("equal", "/hosts")
    cy.get("h1").should("have.text", "Hosts").and("be.visible")
    cy.contains("1 host").should("be.visible")
    cy.contains("tr", "studio-host").within(() => {
      cy.contains("192.0.2.10").should("be.visible")
      cy.contains("0.1.0").should("be.visible")
      cy.contains("online").should("be.visible")
    })
  })

  it("shows the empty state when no hosts are registered", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      body: { items: [], total: 0 },
    }).as("getHosts")

    visitAuthenticatedHosts()
    cy.wait("@getHosts")

    cy.contains("0 hosts").should("be.visible")
    cy.contains("No hosts are available yet.").should("be.visible")
  })

  it("shows the API problem detail when loading hosts fails", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      statusCode: 503,
      headers: { "content-type": "application/problem+json" },
      body: {
        type: "about:blank",
        title: "Service unavailable",
        status: 503,
        detail: "Hosts are temporarily unavailable.",
      },
    }).as("getHosts")

    visitAuthenticatedHosts()
    cy.wait("@getHosts")

    cy.contains("td", "Hosts are temporarily unavailable.").should("be.visible")
  })

  it("opens a host modal, renders type-specific settings, and installs a node", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      fixture: "api/v1/hosts.json",
    }).as("getHosts")
    cy.intercept("GET", "**/api/v1/nodes", {
      fixture: "api/v1/nodes.json",
    }).as("getNodes")
    cy.intercept("GET", "**/api/v1/node-types", {
      fixture: "api/v1/node-types.json",
    }).as("getNodeTypes")
    cy.intercept("POST", "**/api/v1/hosts/*/nodes", {
      statusCode: 201,
      fixture: "api/v1/hosts/20000000-0000-4000-8000-000000000001/nodes.json",
    }).as("installNode")

    visitAuthenticatedHosts()
    cy.wait("@getHosts")

    cy.contains("button", "studio-host").click()
    cy.wait(["@getNodes", "@getNodeTypes"])

    cy.get('[data-slot="dialog-title"]').should("have.text", "Manage studio-host")
    cy.contains("section", "Host details").within(() => {
      cy.contains("192.0.2.10").should("be.visible")
      cy.contains("Linux x86_64").should("be.visible")
    })
    cy.contains("section", "Installed nodes").within(() => {
      cy.contains("Studio microphone").should("be.visible")
      cy.contains(/connected/i).should("be.visible")
      cy.get('button[aria-label="Actions for Studio microphone"]').click()
    })
    cy.get('[data-slot="dropdown-menu-content"]').within(() => {
      cy.contains('[data-slot="dropdown-menu-item"]', "Restart node").should("be.visible")
      cy.contains('[data-slot="dropdown-menu-item"]', "Disable node").should("be.visible")
    })
    cy.get("body").type("{esc}")

    cy.get('button[aria-label="Node type"]').should("contain.text", "Transcriber").click()
    cy.get('[data-slot="select-item"]').contains("Translator").click()
    cy.contains("label", "Source language").should("be.visible")
    cy.contains("label", "Target language").should("be.visible")
    cy.get('button[aria-label="Input"]').should("contain.text", "Live transcriber · studio-host").click()
    cy.get('[data-slot="select-content"]').filter(":visible").within(() => {
      cy.contains('[data-slot="select-item"]', "Live transcriber · studio-host").should("be.visible")
      cy.contains('[data-slot="select-item"]', "Lobby transcriber · studio-host").should("be.visible")
      cy.contains("Studio microphone").should("not.exist")
    })
    cy.get('[data-slot="select-item"]').contains("Live transcriber · studio-host").click()

    cy.get('button[aria-label="Node type"]').click()
    cy.get('[data-slot="select-item"]').contains("Transcriber").click()
    cy.get('button[aria-label="Input"]').should("contain.text", "Studio microphone · studio-host").click()
    cy.get('[data-slot="select-content"]').filter(":visible").within(() => {
      cy.contains('[data-slot="select-item"]', "Studio microphone · studio-host").should("be.visible")
      cy.contains('[data-slot="select-item"]', "Lobby microphone · studio-host").should("be.visible")
      cy.contains("Live transcriber").should("not.exist")
      cy.contains("English → Spanish").should("not.exist")
    })
    cy.get('[data-slot="select-item"]').contains("Lobby microphone · studio-host").click()
    cy.get("#node-name").should("have.value", "live-transcriber")
    cy.get("#node-setting-model").should("have.value", "large-v3")
    cy.get("#node-setting-language").should("have.value", "en")
    cy.get("#node-setting-beamSize").should("have.value", "5")

    cy.contains("button", "Install node").click()
    cy.wait("@installNode").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body).to.deep.equal({
        inputNodeId: "30000000-0000-4000-8000-000000000006",
        type: "transcriber",
        name: "live-transcriber",
        enabled: true,
        settings: {
          model: "large-v3",
          language: "en",
          task: "transcribe",
          beamSize: 5,
          vadEnabled: true,
        },
      })
    })

    cy.get('[role="status"]').should("have.text", "live-transcriber was installed on studio-host.")
    cy.contains("8 nodes").should("exist")
  })
})
