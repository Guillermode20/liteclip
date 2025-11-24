import { render, screen } from '@testing-library/svelte'
import { describe, it, expect } from 'vitest'
import ProgressCard from './ProgressCard.svelte'

describe('ProgressCard', () => {
  it('renders with default values', () => {
    render(ProgressCard)
    
    expect(screen.getByText('// processing')).toBeInTheDocument()
    expect(screen.getByText('0.0%')).toBeInTheDocument()
    
    const progressBar = screen.getByRole('progressbar') || document.querySelector('.progress-fill')
    expect(progressBar).toBeInTheDocument()
    expect(progressBar).toHaveStyle('width: 0%')
  })

  it('displays correct progress percentage', () => {
    render(ProgressCard, { progressPercent: 75.5 })
    
    expect(screen.getByText('75.5%')).toBeInTheDocument()
    
    const progressBar = document.querySelector('.progress-fill')
    expect(progressBar).toHaveStyle('width: 75.5%')
  })

  it('applies compressing class when isCompressing is true', () => {
    render(ProgressCard, { progressPercent: 50, isCompressing: true })
    
    const progressBar = document.querySelector('.progress-fill')
    expect(progressBar).toHaveClass('compressing')
  })

  it('does not apply compressing class when isCompressing is false', () => {
    render(ProgressCard, { progressPercent: 50, isCompressing: false })
    
    const progressBar = document.querySelector('.progress-fill')
    expect(progressBar).not.toHaveClass('compressing')
  })

  it('handles edge case of 100% progress', () => {
    render(ProgressCard, { progressPercent: 100 })
    
    expect(screen.getByText('100.0%')).toBeInTheDocument()
    
    const progressBar = document.querySelector('.progress-fill')
    expect(progressBar).toHaveStyle('width: 100%')
  })
})
