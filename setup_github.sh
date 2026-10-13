#!/bin/bash

# Quick Start Script for GitHub Actions Build
# This script helps you push the code to GitHub to trigger Windows EXE build

set -e

echo "=========================================="
echo "AD Remote Desktop - GitHub Setup Script"
echo "=========================================="
echo ""

# Check if git is installed
if ! command -v git &> /dev/null; then
    echo "❌ Git is not installed. Please install git first:"
    echo "   sudo apt update && sudo apt install git -y"
    exit 1
fi

echo "✅ Git is installed"

# Check if we're in a git repository
if [ ! -d ".git" ]; then
    echo "📦 Initializing git repository..."
    git init
fi

# Configure git user (optional)
if [ -z "$(git config user.name)" ]; then
    echo ""
    read -p "Enter your Git username: " git_username
    git config user.name "$git_username"
fi

if [ -z "$(git config user.email)" ]; then
    read -p "Enter your Git email: " git_email
    git config user.email "$git_email"
fi

echo ""
echo "📝 Adding all files to git..."
git add .

echo ""
echo "📝 Creating initial commit..."
git commit -m "Initial commit: AD Remote Desktop with GitHub Actions CI/CD" || echo "Commit may already exist"

echo ""
echo "🔄 Setting default branch to main..."
git branch -M main

echo ""
echo "=========================================="
echo "Next Steps:"
echo "=========================================="
echo ""
echo "1. Create a new repository on GitHub (do not initialize with README)"
echo ""
echo "2. Run these commands (replace YOUR_USERNAME and REPO_NAME):"
echo ""
echo "   git remote add origin https://github.com/YOUR_USERNAME/REPO_NAME.git"
echo "   git push -u origin main"
echo ""
echo "3. Go to your GitHub repository → Actions tab → Enable GitHub Actions"
echo ""
echo "4. The build will start automatically!"
echo ""
echo "5. After ~15 minutes, download artifacts from the workflow run"
echo ""
echo "For detailed instructions, see: BUILD_WINDOWS_EXE.md"
echo "=========================================="
