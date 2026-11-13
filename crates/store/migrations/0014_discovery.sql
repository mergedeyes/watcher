-- VMs and containers that discovery found on Proxmox VE, with whether an
-- admin set them aside. Whether one is a host already is in the object
-- files (vars.proxmox of the host), not here.
CREATE TABLE discovered_guests (
    discovery  text NOT NULL,
    vmid       int NOT NULL,
    -- qemu or lxc
    kind       text NOT NULL,
    name       text NOT NULL,
    node       text NOT NULL,
    running    boolean NOT NULL,
    -- The last address seen; kept while the guest is stopped.
    address    text,
    ignored    boolean NOT NULL DEFAULT false,
    first_seen timestamptz NOT NULL DEFAULT now(),
    last_seen  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (discovery, vmid)
);
