# 🚀 dataplane-emu: Infrastructure Automation

This directory contains the automation suite to provision and manage a high-performance [AWS Graviton3](https://aws.amazon.com/ec2/instance-types/c7g/) development environment. It is optimized for **SPDK (Storage Performance Development Kit)**, **Kernel Bypass**, and high-speed storage development in C++ or Rust.

## ⚙️ 0. Prerequisites & Tooling

Before running the automation scripts or deploying infrastructure, ensure you have the following tools installed and configured on your local machine:

### A. AWS CLI
The AWS Command Line Interface is required to interact with your AWS account and launch instances.
1. **Install:** Follow the [official AWS CLI installation guide](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html) for your operating system.
2. **Configure:** Open your terminal and run the configuration wizard:
   ```bash
   aws configure
   ```
   You will be prompted to enter your:
   * **AWS Access Key ID**
   * **AWS Secret Access Key**
   * **Default region name** (e.g., `us-east-1` or `us-west-2`)
   * **Default output format** (e.g., `json`)

### B. OpenSSH Client (Windows Users)
Windows users must ensure the native SSH client is enabled to allow the PowerShell automation to connect to the node.

Open PowerShell as Administrator and run:

```PowerShell
Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0
```
Restart your terminal.

### C. Terraform
Terraform is used for any infrastructure-as-code (IaC) state management in this project.
1. **Install:** Follow the [official HashiCorp Terraform installation guide](https://developer.hashicorp.com/terraform/install) for your OS. Here are quick commands for common environments:
   * **macOS (Homebrew):** ```bash
     brew tap hashicorp/tap
     brew install hashicorp/tap/terraform
     ```
   * **Windows (Chocolatey):** ```powershell
     choco install terraform
     ```
     *(Alternatively, download the binary directly from the official guide).*
   * **Linux (Ubuntu/Debian):**
     ```bash
    wget -O - https://apt.releases.hashicorp.com/gpg | sudo gpg --dearmor -o /usr/share/keyrings/hashicorp-archive-keyring.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/hashicorp-archive-keyring.gpg] https://apt.releases.hashicorp.com $(lsb_release -cs) main" | sudo tee /etc/apt/sources.list.d/hashicorp.list
    sudo apt update && sudo apt install terraform
     ```
2. **Verify:** Run `terraform --version` to ensure it is installed correctly.

---

## 📂 Directory Structure

* `provision-graviton.sh`: AWS CLI script to launch a `c7gd` instance with local NVMe.
* `iam-role-setup.sh`: Sets up IAM permissions for passwordless access to AWS services.
* `cloud-init-userdata.yaml`: Bootstraps the OS, installs `clang`, and sets up the SPDK rehydration service.
* `start-graviton.ps1`: Windows PowerShell script to sync local IP with Cloudflare and SSH into the machine.
* `.env.template`: A template for your private infrastructure secrets.

---

## 🔒 1. Security & Secrets Management (Setup)

To prevent leaking sensitive info like your GitHub Personal Access Token or Cloudflare API Tokens, we use environment variables.

**Create your local secret file at the ROOT of the repository:**
```bash
cp scripts/spdk-aws/.env.template .env
```

**Edit `.env`:** Add your actual credentials and instance details.
* **`INSTANCE_ID`**: Your target AWS EC2 Instance ID (e.g., `i-0123456789abcdef0`).
* **`AWS_REGION`**: The region your instance is hosted in (e.g., `us-west-2`).
* **`CF_API_TOKEN`**: Your Cloudflare DNS Token.
* **`CF_ZONE_ID`**: Your Cloudflare Zone ID.
* **`GITHUB_TOKEN`**: Your GitHub Classic PAT.
* **`DEV_DOMAIN`**: Your target domain (e.g., `graviton.myOrg.com`).
* **`DEV_SSH_KEY`**: The absolute local path to your `.pem` key (e.g., `C:\Users\Username\.ssh\spdk-dev-key.pem`).
* **`DEV_USER`**: The default SSH user (e.g., `ec2-user`).

*Note: Double-check that `.env` is added to `.gitignore`. **Never commit this file.***

---

## 🏗️ 2. Provisioning the Infrastructure

### Step A: One-time IAM Setup
Run this once per AWS account to create the necessary execution roles:
```bash
bash scripts/spdk-aws/iam-role-setup.sh
```

### Step B: Launch the Instance
This script finds the latest Amazon Linux 2023 ARM64 AMI and launches the Graviton node:
```bash
cd scripts/spdk-aws/
bash provision-graviton.sh
```

---

## 🛰️ 3. Connecting to the Instance

### From Windows (PowerShell)
The `start-graviton.ps1` script is fully automated. It will dynamically locate your .env file, fetch the live IP of your AWS instance, update Cloudflare DNS in the background, and instantly open an SSH tunnel bypassing local DNS cache.

**Simply run the gateway script from the root of the repository:**
```powershell
.\scripts\spdk-aws\start-graviton.ps1
```

---

## 🛠️ 4. Hybrid Storage Topology

The automation implements a specialized storage layout:
* **EBS Volume (Persistence):** The OS, compiler (`clang`), and SPDK source reside here.
* **NVMe Instance Store (Performance):** The ephemeral Nitro SSD is left raw and automatically bound to the `uio_pci_generic` user-space driver on boot via the `spdk-dev.service`.

### Manual Rehydration
If you need to manually re-bind the hardware or check hugepage allocation:
```bash
sudo /usr/local/bin/rehydrate-spdk.sh
```

---

## 🧪 5. Verification

Once logged in, verify the Kernel Bypass is active by running the SPDK identify tool:
```bash
cd ~/project/spdk
sudo LD_LIBRARY_PATH=./build/lib ./build/bin/spdk_nvme_identify -r "trtype:PCIe traddr:0000:00:1f.0"
```

If you see "Amazon EC2 NVMe Instance Storage", the bypass is successful.
